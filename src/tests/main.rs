use crate::test_support::*;
// SPDX-License-Identifier: MIT

// Test-only constructors and helpers kept out of production files so the
// coverage gate can exclude test code by filename alone.
impl ChartPoint {
    fn new(value: Option<u64>, tone: Tone) -> Self {
        Self {
            value,
            tone,
            observed_at: SystemTime::now(),
            usual: None,
        }
    }
}

impl crate::analysis::Speeds {
    /// The usual speed over every earlier sample.
    fn level(&self) -> Option<f64> {
        // A lone request with the context the test samples hold.
        let lone = crate::analysis::Work {
            requests: 1,
            context: Some(70_000),
        };
        self.usual(lone).level()
    }
}

fn chart_plot_values(
    history: &VecDeque<ChartPoint>,
    metric: ChartMetric,
    plot_height: usize,
) -> Vec<Option<u64>> {
    chart_plot_values_scaled(
        history,
        metric,
        plot_height,
        chart_scale(history, metric, history.len()),
    )
}

/** Built-in thresholds, for tests that are not exercising configuration. */
pub(super) fn defaults() -> Thresholds {
    Thresholds::default()
}

/**
 * A config file that sets every supported field to a value that differs
 * from the built-in default and survives normalization unchanged.
 */
pub(crate) const CONFIG_WITH_EVERY_FIELD_SET: &str = r#"{
    "interval": 7,
    "history": 1234,
    "omx": { "host": "10.1.2.3", "port": 9999 },
    "memory_warn_load": 55,
    "memory_critical_load": 77,
    "gpu_warn_load": 44,
    "gpu_critical_load": 66,
    "gpu_warn_exit": 33,
    "swap_warn_rate": 3145728,
    "swap_critical_rate": 25165824,
    "swap_warn_exit": 4194304,
    "compression_warn_rate": 100663296,
    "compression_warn_exit": 50331648,
    "alarms": false,
    "journal": {
        "prompts": false, "llm": false, "queue": false, "memory": false,
        "paging": false, "gpu": false, "thermal": false
    }
}"#;

pub(super) fn test_app(tab: usize) -> App {
    let (app, _views) = test_app_with_sender(tab);
    app
}

pub(super) fn test_app_with_sender(tab: usize) -> (App, Sender<CollectorView>) {
    let (commands, _command_receiver) = mpsc::channel();
    let (view_sender, views) = mpsc::channel();
    let app = App {
        collector: CollectorView {
            current: Sample {
                total_memory: 1,
                ..Sample::default()
            },
            generation_history: VecDeque::new(),
            prefill_history: VecDeque::new(),
            cache_history: VecDeque::new(),
            disk_history: VecDeque::new(),
            decode_reads: Default::default(),
            load_history: VecDeque::new(),
            composition_history: VecDeque::new(),
            swap_history: VecDeque::new(),
            compression_history: VecDeque::new(),
            platform: Platform::MacOs,
            gpu_history: VecDeque::new(),
            signals: VecDeque::new(),
            request_history: request_history::History::default(),
            operator_history: operator_history::History::default(),
        },
        sampler: Sampler {
            commands,
            views,
            handle: None,
        },
        interval: Duration::from_secs(1),
        paused: false,
        tab,
        top_sort: TopSort::Rss,
        top_filter: String::new(),
        top_filtering: false,
        top_selected: 0,
        journal_filter: JournalFilter::All,
        journal_scroll: 0,
        request_scroll: 0,
        charts: chart_navigation::Navigation::default(),
        gpu_selected: 0,
        help: false,
        diagnostics_open: false,
        diagnostics_scroll: 0,
        diagnostics_max_scroll: Default::default(),
        quit: false,
        quit_prompt: false,
        sampler_disconnected: false,
        alert: None,
        alert_bells: 0,
        bell: || {},
        critical_episode: false,
        alarms: true,
        thresholds: Thresholds::default(),
        settings: crate::settings::Settings::new(std::path::PathBuf::from(
            "/nonexistent/mlxtop-test/config.json",
        )),
        last_click: None,
    };
    (app, view_sender)
}

pub(super) fn view_with_impact(impact: &str, updated: &str) -> CollectorView {
    CollectorView {
        current: Sample {
            impact: impact.into(),
            updated: updated.into(),
            ..Sample::default()
        },
        generation_history: VecDeque::new(),
        prefill_history: VecDeque::new(),
        cache_history: VecDeque::new(),
        disk_history: VecDeque::new(),
        decode_reads: Default::default(),
        load_history: VecDeque::new(),
        composition_history: VecDeque::new(),
        swap_history: VecDeque::new(),
        compression_history: VecDeque::new(),
        platform: Platform::MacOs,
        gpu_history: VecDeque::new(),
        signals: VecDeque::new(),
        request_history: request_history::History::default(),
        operator_history: operator_history::History::default(),
    }
}

pub(super) fn render_app(app: &App, width: u16, height: u16) -> String {
    render_view(width, height, |frame| app.draw(frame))
}

pub(super) fn populate_dashboard_fixture(app: &mut App) {
    let now = SystemTime::now();
    app.collector.current = Sample {
        runtime: crate::runtime_diagnostics::RuntimeReport {
            provider: Some("oMLX".into()),
            selection: "process detection",
            endpoint: "http://127.0.0.1:8080".into(),
            credentials_present: true,
            connected: true,
            attempted_at: Some(now),
            succeeded_at: Some(now),
            ..Default::default()
        },
        impact: "LLM READY".into(),
        correlation: CorrelationInsight {
            direction: ThroughputDirection::Flat,
            ..Default::default()
        },
        total_memory: 36 * 1024 * MIB,
        availability: Some(93),
        resident_memory: Some((36 * 1024 * MIB * 57).div_ceil(100)),
        pressure: "GREEN".into(),
        pressure_meaning: "normal".into(),
        pressure_tone: Tone::Green,
        vm_available: true,
        swap_available: true,
        rate_ready: true,
        swap_total: 2 * 1024 * MIB,
        swap_used: 1100 * MIB,
        swap_in: 4096,
        wired: 3277 * MIB,
        anonymous: 12_698 * MIB,
        compressor: 1638 * MIB,
        compressed_logical: 4710 * MIB,
        file_backed: 3380 * MIB,
        compress: 3 * MIB,
        decompress: 3 * MIB / 2,
        gpu_util: Some(95),
        gpu_in_use: Some(18 * 1024 * MIB),
        llm_provider: "oMLX".into(),
        llm_model: "Qwen3.8-27B-oQ4e-mtp".into(),
        llm_status: "generating".into(),
        llm_source: TelemetrySource::Live,
        llm_observed_at: Some(now),
        llm_generation_tps: Some(35.5),
        llm_generation_tps_live: true,
        llm_prefill_tps: Some(215.3),
        llm_active_requests: Some(1),
        llm_waiting_requests: Some(0),
        llm_count: 2,
        llm_processes: vec![
            LlmProcess {
                pid: 41441,
                name: "omlx-server".into(),
                command: "/usr/local/bin/omlx-server --port 8000".into(),
                rss: 16 * 1024 * MIB,
                cpu: 0.8,
                memory_percent: Some(44.4),
                state: "S".into(),
                pageins: Some(25),
                pagein_rate: Some(0.0),
            },
            LlmProcess {
                pid: 42002,
                name: "llama-server".into(),
                command: "/opt/llama-server --model small-model.gguf --port 8080".into(),
                rss: 2 * 1024 * MIB,
                cpu: 12.0,
                memory_percent: Some(5.6),
                state: "R".into(),
                pageins: Some(10),
                pagein_rate: Some(2.0),
            },
        ],
        llm_pid: 41441,
        llm_prompt_tokens: Some(32768),
        llm_output_tokens: Some(1200),
        llm_cache_efficiency: Some(61.9),
        llm_prefix_hit_rate: Some(39.9),
        llm_rss: 18 * 1024 * MIB,
        llm_cpu: 12.8,
        process_memory: Some(process_memory::Reading {
            pid: 41441,
            started: 1,
            resident: 16 * 1024 * MIB,
            footprint: 19 * 1024 * MIB,
            peak: 30 * 1024 * MIB,
            disk_read: 0,
            disk_written: 0,
            at: Instant::now(),
        }),
        process_memory_growth: Some(0),
        ..Sample::default()
    };
    app.collector.current.metal.device_name = Some("Apple M4 Max".into());
    app.collector.current.metal.gpu_cores = Some(32);
    app.collector.current.thermal = "no warning".into();
    app.collector.current.gpu_alloc = Some(24 * 1024 * MIB);
    app.collector.current.mlx.recommended_working_set = Some(27 * 1024 * MIB);
    for index in 0..100 {
        for (history, metric, value) in [
            (
                &mut app.collector.generation_history,
                ChartMetric::Generation,
                320 + index % 8 * 10,
            ),
            (
                &mut app.collector.prefill_history,
                ChartMetric::Prefill,
                1950 + index % 9 * 40,
            ),
            (
                &mut app.collector.gpu_history,
                ChartMetric::Gpu,
                [60, 78, 95][index as usize / 10 % 3],
            ),
            (
                &mut app.collector.load_history,
                ChartMetric::Memory,
                55 + index % 8,
            ),
            (
                &mut app.collector.swap_history,
                ChartMetric::Swap,
                index % 7 * 1024,
            ),
            (
                &mut app.collector.cache_history,
                ChartMetric::Cache,
                55 + index % 15,
            ),
            (
                &mut app.collector.compression_history,
                ChartMetric::Compression,
                (index % 6 + 1) * 768 * 1024,
            ),
        ] {
            // Grade exactly as the collector does.
            let tone = match metric {
                ChartMetric::Memory => app.collector.current.pressure_tone,
                // Speeds have no grade: usual here, so green.
                ChartMetric::Generation | ChartMetric::Prefill => Tone::Green,
                _ => metric.tone(value, defaults()),
            };
            let mut point = ChartPoint::new(Some(value), tone);
            point.observed_at = now - Duration::from_secs(119 - index);
            history.push_back(point);
        }
        let mut historical = app.collector.current.clone();
        if let Some(memory) = historical.process_memory.as_mut() {
            memory.footprint = 17 * 1024 * MIB + (index.min(80) * 2 * 1024 * MIB / 80);
        }
        app.collector.operator_history.observe(&historical, 120);
    }
    *app.collector.generation_history.back_mut().unwrap() = ChartPoint::new(Some(355), Tone::Green);
    *app.collector.prefill_history.back_mut().unwrap() = ChartPoint::new(None, Tone::Muted);
    *app.collector.gpu_history.back_mut().unwrap() = ChartPoint::new(Some(95), Tone::Red);
    *app.collector.load_history.back_mut().unwrap() = ChartPoint::new(Some(57), Tone::Green);
    *app.collector.swap_history.back_mut().unwrap() = ChartPoint::new(Some(4096), Tone::Green);
    *app.collector.compression_history.back_mut().unwrap() =
        ChartPoint::new(Some(9 * MIB / 2), Tone::Green);
    for (index, prompt) in [500, 729, 837, 12000, 20000, 32768].into_iter().enumerate() {
        let request = domain::RequestUsage {
            provider: "oMLX".into(),
            model: "Qwen3.8-27B-oQ4e-mtp".into(),
            id: format!("preview-{index}"),
            prompt,
            cached: Some(prompt / 2),
            output: Some(1200),
            completed: index < 5,
            queued: false,
            prefilled: None,
            idle_for: None,
            ttft_ms: None,
            output_tps: Some(35.5 + (5 - index) as f64),
            observed_at: Some(now - Duration::from_secs((5 - index) as u64 * 10)),
        };
        app.collector
            .request_history
            .observe(std::slice::from_ref(&request));
        if index == 5 {
            app.collector.current.llm_requests = vec![request];
        }
    }
}

pub(super) fn populate_single_idle_fixture(app: &mut App) {
    populate_dashboard_fixture(app);
    let now = SystemTime::now();
    let sample = &mut app.collector.current;
    sample.llm_status = "idle".into();
    sample.llm_generation_tps = Some(35.7);
    sample.llm_generation_tps_live = false;
    sample.llm_prefill_tps = Some(120.6);
    sample.llm_prefill_tps_live = false;
    sample.llm_cache_efficiency = Some(62.8);
    sample.llm_cache_interval_efficiency = None;
    sample.llm_prefix_hit_rate = Some(39.7);
    sample.llm_prompt_tokens = None;
    sample.llm_output_tokens = None;
    sample.llm_active_requests = Some(0);
    sample.llm_waiting_requests = Some(0);
    sample.llm_requests.clear();
    sample.availability = Some(93);
    sample.resident_memory = Some(30 * 1024 * MIB);
    sample.gpu_util = Some(0);
    sample.swap_in = 0;
    sample.swap_out = 0;
    app.collector.request_history = request_history::History::default();
    app.collector
        .request_history
        .observe(&[domain::RequestUsage {
            provider: sample.llm_provider.clone(),
            model: sample.llm_model.clone(),
            id: "single-idle-preview".into(),
            prompt: 2947,
            cached: None,
            output: Some(346),
            completed: false,
            queued: false,
            prefilled: None,
            idle_for: None,
            ttft_ms: None,
            output_tps: Some(40.0),
            observed_at: Some(now - Duration::from_secs(25)),
        }]);
    app.collector.operator_history = operator_history::History::default();
    for history in [
        &mut app.collector.generation_history,
        &mut app.collector.prefill_history,
        &mut app.collector.cache_history,
        &mut app.collector.load_history,
        &mut app.collector.swap_history,
        &mut app.collector.gpu_history,
    ] {
        history.clear();
    }
    for index in 0..120 {
        let active = (86..95).contains(&index);
        sample.llm_active_requests = Some(u64::from(active));
        app.collector.operator_history.observe(sample, 120);
        for (history, metric, value) in [
            (
                &mut app.collector.generation_history,
                ChartMetric::Generation,
                active.then_some(if index < 92 { 440 } else { 400 }),
            ),
            (
                &mut app.collector.prefill_history,
                ChartMetric::Prefill,
                None,
            ),
            (
                &mut app.collector.cache_history,
                ChartMetric::Cache,
                (index == 94).then_some(0),
            ),
            (
                &mut app.collector.load_history,
                ChartMetric::Memory,
                Some(if active { 89 } else { 83 }),
            ),
            (&mut app.collector.swap_history, ChartMetric::Swap, Some(0)),
            (
                &mut app.collector.gpu_history,
                ChartMetric::Gpu,
                Some(if active { 90 } else { 0 }),
            ),
        ] {
            if metric == ChartMetric::Memory {
                push_history_with_tone(history, value, sample.pressure_tone, 120);
            } else {
                push_history(history, value, metric, 120, defaults());
            }
            history.back_mut().unwrap().observed_at = now - Duration::from_secs(119 - index);
        }
    }
}

#[test]
fn flat_panels_keep_units_and_explain_a_zoomed_gap() {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    for (width, height) in [(80, 24), (100, 40), (180, 50)] {
        let screen = render_app(&app, width, height);
        assert!(screen.contains("Qwen3.8-27B-oQ4e-mtp"));
        assert!(screen.contains("Apple M4 Max"));
        assert!(
            !screen.contains("THERMAL no warning"),
            "normal thermals are not news"
        );
        assert!(
            !screen.contains("┌ throughput"),
            "no extra throughput panel"
        );
        assert!(!screen.contains("memory · system / process"));
        let regions = app.charts.regions.borrow().clone();
        for chart in [Chart::Generation, Chart::Prefill] {
            let area = regions.iter().find(|(key, _)| *key == chart).unwrap().1;
            let panel = render_view(area.width, area.height, |frame| {
                app.render_indicator_chart(
                    frame,
                    frame.area(),
                    if chart == Chart::Generation {
                        "generation"
                    } else {
                        "prefill"
                    },
                    if chart == Chart::Generation {
                        &app.collector.generation_history
                    } else {
                        &app.collector.prefill_history
                    },
                    if chart == Chart::Generation {
                        ChartMetric::Generation
                    } else {
                        ChartMetric::Prefill
                    },
                );
            });
            assert!(
                panel.contains("tok/s"),
                "missing rate units at {width} columns\n{panel}"
            );
        }
    }
    app.collector.generation_history = VecDeque::from([ChartPoint::new(Some(355), Tone::Cyan)]);
    app.collector
        .generation_history
        .extend(std::iter::repeat_n(ChartPoint::new(None, Tone::Muted), 20));
    app.charts.focused = Chart::Generation;
    for _ in 0..3 {
        app.charts.change_zoom(true);
    }
    let gap = render_view(60, 10, |frame| {
        app.render_indicator_chart(
            frame,
            frame.area(),
            "generation",
            &app.collector.generation_history,
            ChartMetric::Generation,
        )
    });
    assert!(gap.contains("No samples in this window"));
}

#[test]
fn memory_chart_colors_follow_pressure_even_with_high_cache_occupancy() {
    let mut app = test_app(0);
    for (resident, state, label, tone) in [
        (87, "GREEN", "normal", Tone::Green),
        (100, "GREEN", "normal", Tone::Green),
        (87, "YELLOW", "watch", Tone::Yellow),
        (7, "RED", "critical", Tone::Red),
        (87, "UNKNOWN", "unavailable", Tone::Muted),
    ] {
        app.collector.current.pressure = state.into();
        app.collector.current.pressure_tone = tone;
        app.collector.load_history.clear();
        push_history_with_tone(&mut app.collector.load_history, Some(resident), tone, 120);
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(51, 13)).unwrap();
        terminal
            .draw(|frame| {
                app.render_indicator_chart(
                    frame,
                    frame.area(),
                    "memory",
                    &app.collector.load_history,
                    ChartMetric::Memory,
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let headline: String = (0..51).map(|x| buffer[(x, 0)].symbol()).collect();
        let reading: String = (0..51).map(|x| buffer[(x, 1)].symbol()).collect();
        let footer: String = (0..51).map(|x| buffer[(x, 12)].symbol()).collect();
        // Pressure is the verdict in the reading slot; occupancy supports it.
        let pressure = format!("PRESSURE {label}");
        assert!(headline.contains(&pressure), "{headline}");
        let at = headline.find(&pressure).unwrap();
        let column = headline[..at].chars().count() as u16;
        assert_eq!(buffer[(column, 0)].fg, tone.color());
        let resident_label = format!("{resident}% resident");
        assert!(reading.contains(&resident_label), "{reading}");
        let at = reading.find(&resident_label).unwrap();
        assert_eq!(
            buffer[(reading[..at].chars().count() as u16, 1)].fg,
            CYAN,
            "resident percentage is occupancy; pressure has a separate severity label"
        );
        assert!(!footer.contains("Includes file cache"), "no filler footer");
        let sample_cells: Vec<_> = buffer
            .content
            .iter()
            .filter(|cell| cell.symbol() == "●")
            .collect();
        assert!(!sample_cells.is_empty());
        // The occupancy trace is graded by the captured pressure state.
        assert!(sample_cells.iter().all(|cell| cell.fg == tone.color()));
        let mut compact = Terminal::new(ratatui::backend::TestBackend::new(40, 4)).unwrap();
        compact
            .draw(|frame| {
                app.render_indicator_chart(
                    frame,
                    frame.area(),
                    "memory",
                    &app.collector.load_history,
                    ChartMetric::Memory,
                )
            })
            .unwrap();
        let top: String = (0..40)
            .map(|x| compact.backend().buffer()[(x, 0)].symbol())
            .collect();
        assert!(top.contains(&pressure), "{top}");
    }
}

#[test]
fn memory_history_preserves_pressure_changes_and_missing_samples() {
    let mut history = VecDeque::new();
    for (resident, tone) in [
        (Some(87), Tone::Green),
        (Some(87), Tone::Red),
        (None, Tone::Green),
        (Some(90), Tone::Green),
    ] {
        push_history_with_tone(&mut history, resident, tone, 120);
    }
    assert_eq!(
        history.iter().map(|point| point.tone).collect::<Vec<_>>(),
        [Tone::Green, Tone::Red, Tone::Muted, Tone::Green]
    );
    for resident in [0, 70, 85, 100] {
        assert_eq!(
            chart_transition_tone(
                ChartMetric::Memory,
                resident,
                Tone::Green,
                Tone::Green,
                defaults()
            ),
            Tone::Green
        );
        assert_eq!(
            chart_transition_tone(
                ChartMetric::Memory,
                resident,
                Tone::Green,
                Tone::Red,
                defaults()
            ),
            Tone::Red
        );
        assert_eq!(
            chart_transition_tone(
                ChartMetric::Memory,
                resident,
                Tone::Red,
                Tone::Muted,
                defaults()
            ),
            Tone::Muted
        );
    }
}

#[test]
fn paging_connectors_never_outrank_the_points_they_join() {
    let heavy = 900 * 1024 * 1024;
    let tone = |previous, current| {
        chart_transition_tone(ChartMetric::Swap, heavy, previous, current, defaults())
    };
    // A model load on a roomy machine: two green points, so no red edge.
    assert_eq!(tone(Tone::Green, Tone::Green), Tone::Green);
    // Real heavy paging keeps its red, whichever side it is on.
    assert_eq!(tone(Tone::Green, Tone::Red), Tone::Red);
    assert_eq!(tone(Tone::Red, Tone::Green), Tone::Red);
    assert_eq!(tone(Tone::Yellow, Tone::Green), Tone::Yellow);
    // Below the warning rate the value still grades a connector down.
    assert_eq!(
        chart_transition_tone(ChartMetric::Swap, 0, Tone::Red, Tone::Red, defaults()),
        Tone::Green
    );
}

#[test]
fn compact_resource_panels_use_capacity_bars_without_false_axes() {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    for (metric, history, label) in [
        (
            ChartMetric::Memory,
            &app.collector.load_history,
            "57% resident",
        ),
        (ChartMetric::Gpu, &app.collector.gpu_history, "95%"),
    ] {
        let screen = render_view(40, 4, |frame| {
            app.render_indicator_chart(frame, frame.area(), "metric", history, metric)
        });
        assert!(screen.contains(label), "{screen}");
        assert!(!screen.contains("100"));
        // The row under the gauge is a sparkline of the history, not a hint.
        assert!(!screen.contains("Enter: history"), "{screen}");
    }
    // Memory keeps its exact reading with the bar instead of a flat 3-row trace.
    let screen = render_view(40, 5, |frame| {
        app.render_indicator_chart(
            frame,
            frame.area(),
            "memory",
            &app.collector.load_history,
            ChartMetric::Memory,
        )
    });
    for label in ["20.5 GiB / 36.0 GiB", "57% resident", "wired 3.2 GiB"] {
        assert!(screen.contains(label), "missing {label}\n{screen}");
    }
    assert!(!screen.contains("50%"), "no percentage axis\n{screen}");
    let screen = render_view(40, 4, |frame| {
        app.render_indicator_chart(
            frame,
            frame.area(),
            "paging / I/O",
            &app.collector.swap_history,
            ChartMetric::Swap,
        )
    });
    assert!(screen.contains("swap"));
    assert!(screen.contains("1.1/2.0 GiB 54%"));
    app.collector.cache_history.clear();
    let screen = render_view(45, 8, |frame| {
        app.render_indicator_chart(
            frame,
            frame.area(),
            "cache",
            &app.collector.cache_history,
            ChartMetric::Cache,
        )
    });
    assert!(screen.contains("since start 61.9%"));
    assert!(screen.contains("No cache samples yet"));
    // Narrow panels keep the whole message rather than clipping "yet".
    let narrow = render_view(20, 8, |frame| {
        app.render_indicator_chart(
            frame,
            frame.area(),
            "cache",
            &app.collector.cache_history,
            ChartMetric::Cache,
        )
    });
    assert!(narrow.contains("No cache samples"), "{narrow}");
    assert!(!narrow.contains("samples y"), "{narrow}");
}

#[test]
fn idle_charts_label_last_samples_separately_from_session_averages() {
    let mut app = test_app(0);
    // A changed cadence must not recalculate the age of retained samples.
    app.interval = Duration::from_secs(5);
    app.collector.current = Sample {
        llm_source: TelemetrySource::Live,
        llm_status: "idle".into(),
        llm_generation_tps: Some(35.7),
        llm_prefill_tps: Some(120.6),
        llm_cache_efficiency: Some(62.8),
        llm_prefix_hit_rate: Some(39.7),
        ..Sample::default()
    };
    for (metric, name, last, labels) in [
        (
            ChartMetric::Generation,
            "generation",
            Some(400),
            vec!["LAST 40.0 tok/s · 25s old", "tok/s"],
        ),
        (
            ChartMetric::Cache,
            "cache",
            Some(0),
            vec!["LAST 0% · 25s old", "since start 62.8%", "prefix 39.7%"],
        ),
        (
            ChartMetric::Prefill,
            "prefill",
            None,
            vec!["Idle · no rate samples yet", "tok/s"],
        ),
    ] {
        let mut point = ChartPoint::new(last, Tone::Cyan);
        point.observed_at = SystemTime::now() - Duration::from_secs(25);
        let mut history = VecDeque::from([point]);
        history.extend(std::iter::repeat_n(ChartPoint::new(None, Tone::Muted), 25));
        let screen = render_view(90, 8, |frame| {
            app.render_indicator_chart(frame, frame.area(), name, &history, metric)
        });
        for label in labels {
            assert!(screen.contains(label), "missing {label}\n{screen}");
        }
        if metric == ChartMetric::Generation {
            // A bar of filled cells (blank in text) cutting the guides, not a point.
            assert!(screen.contains("┄ ┄") && screen.contains("─ ─"), "{screen}");
            assert!(!screen.contains('●'), "{screen}");
        } else if chart_form(metric) == ChartForm::Bars {
            assert!(!screen.contains('●'), "bars, not points\n{screen}");
        } else {
            assert_eq!(screen.contains('●'), last.is_some(), "{screen}");
        }
        if last.is_none() {
            assert!(!screen.contains("window avg"));
        }
    }
}

#[test]
fn the_prefill_chart_shows_how_far_the_prefill_has_got() {
    let mut app = test_app(0);
    app.collector.current = Sample {
        llm_source: TelemetrySource::Live,
        llm_status: "prefilling".into(),
        llm_prefill_tps: Some(4100.0),
        llm_prefill_tps_live: true,
        llm_prefill_tokens_done: Some(40960),
        llm_prefill_tokens_expected: Some(61455),
        ..Sample::default()
    };
    let history = VecDeque::from([ChartPoint::new(Some(41000), Tone::Cyan)]);
    let screen = render_view(90, 8, |frame| {
        app.render_indicator_chart(
            frame,
            frame.area(),
            "prefill",
            &history,
            ChartMetric::Prefill,
        )
    });
    assert!(screen.contains("41.0k / 61.5k · 66%"), "{screen}");
    assert!(screen.contains("LIVE 4100.0 tok/s"), "{screen}");
}

#[test]
fn native_last_prefill_labels_itself_last_instead_of_live() {
    let mut app = test_app(0);
    app.collector.current = Sample {
        llm_provider: "mlx-serve".into(),
        llm_prefill_tps: Some(2134.4),
        ..Sample::default()
    };
    let history = VecDeque::new();
    let screen = render_view(90, 8, |frame| {
        app.render_indicator_chart(
            frame,
            frame.area(),
            "prefill",
            &history,
            ChartMetric::Prefill,
        )
    });
    assert!(
        screen.contains("LAST PREFILL 2134.4 tok/s"),
        "the held reading is labeled last, not live\n{screen}"
    );
    assert!(!screen.contains("LIVE"), "{screen}");
}

#[test]
fn consolidated_dashboard_keeps_metrics_once_and_gpu_bands_visible() {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    for (width, height) in [(80, 24), (100, 40), (180, 50)] {
        let screen = render_app(&app, width, height);
        for label in [
            "Qwen3.8-27B-oQ4e-mtp",
            "prompt load",
            "memory",
            "GPU",
            "paging",
            "cache",
            "queue",
            "32.8k▲",
        ] {
            assert!(
                screen.contains(label),
                "missing {label} at {width}×{height}\n{screen}"
            );
        }
        assert_eq!(screen.matches("┌ generation").count(), 1);
        assert!(!screen.contains("cache / queue"));
        let regions = app.charts.regions.borrow();
        let compression = regions
            .iter()
            .find(|(chart, _)| *chart == Chart::Compression)
            .unwrap()
            .1;
        let queue = regions
            .iter()
            .find(|(chart, _)| *chart == Chart::Queue)
            .unwrap()
            .1;
        // Compression and queue stack in the grid's third column.
        assert_eq!(compression.x, queue.x);
        assert_eq!(compression.width, queue.width);
        assert!(compression.bottom() <= queue.y);
        assert!(screen.contains("active 1"));
        assert!(screen.contains("waiting 0"));
        assert!(!screen.contains("DIAGNOSIS"));
        assert!(!screen.contains("GPU / COMPUTE"));
        assert!(!screen.contains("0–100 log"));
    }
    for (value, tone) in [
        (74, Tone::Green),
        (75, Tone::Yellow),
        (89, Tone::Yellow),
        (90, Tone::Red),
        (100, Tone::Red),
    ] {
        assert_eq!(ChartMetric::Gpu.tone(value, defaults()), tone);
    }
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(90, 12)).unwrap();
    terminal
        .draw(|frame| {
            app.render_indicator_chart(
                frame,
                frame.area(),
                "GPU",
                &app.collector.gpu_history,
                ChartMetric::Gpu,
            )
        })
        .unwrap();
    for color in [GREEN, YELLOW, RED] {
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.fg == color && matches!(cell.symbol(), "━" | "┃")),
            "missing GPU load band {color:?}"
        );
    }
}

#[test]
#[ignore = "interactive fixture for terminal layout review"]
fn dashboard_terminal_preview() {
    let (mut app, _views) = test_app_with_sender(0);
    populate_dashboard_fixture(&mut app);
    let preview_state = env::var("MLXTOP_PREVIEW_STATE").unwrap_or_default();
    if preview_state == "nvidia" {
        app = nvidia_app(4);
    }
    if preview_state == "diagnostics" {
        app.diagnostics_open = true;
    }
    if preview_state == "critical" {
        app.collector.current.pressure = "RED".into();
        app.collector.current.pressure_tone = Tone::Red;
    }
    if preview_state == "alert" {
        app.collector.current.pressure = "RED".into();
        app.collector.current.pressure_tone = Tone::Red;
        app.alert = Some(ActiveAlert {
            state: "MEMORY BOTTLENECK".into(),
            summary: "Critical memory pressure · paging 18.0 MiB/s".into(),
            time: "16:30:52".into(),
        });
    }
    if preview_state == "single-idle" {
        populate_single_idle_fixture(&mut app);
    }
    if matches!(preview_state.as_str(), "idle" | "empty") {
        app.collector.current.llm_status = "idle".into();
        app.collector.current.llm_active_requests = Some(0);
        app.collector.current.llm_prompt_tokens = None;
        app.collector.current.llm_output_tokens = None;
        app.collector.current.llm_generation_tps_live = false;
        app.collector.current.llm_requests.clear();
        app.collector
            .operator_history
            .observe(&app.collector.current, 120);
        app.collector
            .generation_history
            .push_back(ChartPoint::new(None, Tone::Muted));
        app.collector
            .prefill_history
            .push_back(ChartPoint::new(None, Tone::Muted));
        if preview_state == "empty" {
            app.collector.request_history = request_history::History::default();
        }
    }
    app.collector.signals = [
        (
            "16:30:00",
            "LLM READY",
            "oMLX connected · Qwen3.8-27B-oQ4e-mtp loaded",
            Tone::Green,
        ),
        (
            "16:30:06",
            "PROMPT",
            "Request observed · 12,000 input tokens · cached 50%",
            Tone::Cyan,
        ),
        (
            "16:30:15",
            "MEMORY",
            "Process footprint increased to 18.4 GiB · pressure normal",
            Tone::Cyan,
        ),
        (
            "16:30:22",
            "PROMPT",
            "Request observed · 20,000 input tokens · cached 50%",
            Tone::Cyan,
        ),
        (
            "16:30:29",
            "QUEUE",
            "1 active request · 0 waiting",
            Tone::Cyan,
        ),
        (
            "16:30:33",
            "PROMPT",
            "Request observed · 32,768 input tokens · cached 50%",
            Tone::Cyan,
        ),
        (
            "16:30:38",
            "GPU",
            "GPU load reached 95% · throughput stable",
            Tone::Yellow,
        ),
        (
            "16:30:45",
            "MEMORY",
            "Process footprint stable at 19.0 GiB · no paging pressure",
            Tone::Green,
        ),
    ]
    .into_iter()
    .map(|(time, state, summary, tone)| SignalEvent {
        time: time.into(),
        recorded_at: SystemTime::now(),
        kind: EventKind::from_state(state),
        state: state.into(),
        summary: summary.into(),
        tone,
    })
    .collect();
    if preview_state == "single-idle" {
        app.collector.signals.clear();
        for (state, summary) in [
            (
                "PROMPT",
                "Request observed · 2,947 input tokens · 346 output tokens",
            ),
            (
                "LLM READY",
                "Request finished · 35.7 tok/s average · ready for next request",
            ),
            ("GPU", "GPU load returned to 0% · idle"),
        ] {
            app.collector.signals.push_back(SignalEvent {
                time: "16:31:33".into(),
                recorded_at: SystemTime::now(),
                kind: EventKind::from_state(state),
                state: state.into(),
                summary: summary.into(),
                tone: Tone::Green,
            });
        }
    }
    // Previews show local clocks like a sampled host, where `date` reports the zone.
    app.collector.current.utc_offset = Some(0);
    enable_raw_mode().unwrap();
    let _guard = TerminalGuard::new();
    let mut output = stdout();
    execute!(
        output,
        EnterAlternateScreen,
        EnableMouseCapture,
        crossterm::cursor::Hide
    )
    .unwrap();
    let mut terminal = Terminal::new(CrosstermBackend::new(output)).unwrap();
    run_app(&mut terminal, &mut app, &mut next_terminal_event).unwrap();
}

// Exercise the NVIDIA layout on development hosts too; production routing
// is covered separately and must never enable it on a non-Linux target.
fn render_nvidia_overview(app: &App, width: u16, height: u16) -> String {
    render_view(width, height, |frame| {
        app.draw_header(frame, Rect::new(0, 0, width, 1));
        app.draw_gpu_overview(frame, Rect::new(0, 1, width, height - 2));
        app.draw_controls(frame, Rect::new(0, height - 1, width, 1));
    })
}

pub(super) fn render_view(width: u16, height: u16, draw: impl FnOnce(&mut Frame)) -> String {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend should initialize");
    terminal.draw(draw).expect("application should render");
    let mut rendered = String::new();
    for y in 0..height {
        for x in 0..width {
            rendered.push_str(
                terminal
                    .backend()
                    .buffer()
                    .cell((x, y))
                    .expect("rendered cell should exist")
                    .symbol(),
            );
        }
        rendered.push('\n');
    }
    rendered
}

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
fn paging_distinguishes_zero_traffic_from_missing_and_past_activity() {
    let mut app = test_app(0);
    app.charts.expanded = true;
    let draw = |history: &VecDeque<ChartPoint>| {
        render_view(90, 12, |frame| {
            app.render_indicator_chart(
                frame,
                frame.area(),
                "paging / I/O",
                history,
                ChartMetric::Swap,
            );
        })
    };
    let mut history = VecDeque::from([ChartPoint::new(Some(0), Tone::Green)]);
    let idle = draw(&history);
    assert!(idle.contains("No paging traffic in this window"));
    assert!(!idle.contains("1 B/s"));
    assert!(idle.contains("0 B/s"));
    // A measured zero marks the baseline in its grade; the empty guide is muted.
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(90, 12)).unwrap();
    terminal
        .draw(|frame| {
            app.render_indicator_chart(
                frame,
                frame.area(),
                "paging / I/O",
                &history,
                ChartMetric::Swap,
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert!(
        buffer
            .content()
            .iter()
            .any(|cell| cell.symbol() == "─" && cell.fg == GREEN),
        "retain measured zero"
    );
    history.push_front(ChartPoint::new(Some(4096), Tone::Green));
    let past_activity = draw(&history);
    assert!(!past_activity.contains("No paging traffic"));
    assert!(past_activity.contains("KiB/s"));
    history = VecDeque::from([ChartPoint::new(None, Tone::Muted)]);
    let missing = draw(&history);
    assert!(missing.contains("No samples yet"));
    assert!(!missing.contains("No paging traffic"));
    history.push_back(ChartPoint::new(Some(1), Tone::Green));
    assert!(!draw(&history).contains("No paging traffic"));
    // An old spike outside the visible range must not create an idle axis.
    history.extend(std::iter::repeat_n(
        ChartPoint::new(Some(0), Tone::Green),
        100,
    ));
    assert!(draw(&history).contains("No paging traffic in this window"));
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
fn linux_pressure_follows_load_and_stall() {
    assert_eq!(linux_pressure_state(10, None, defaults()).0, "GREEN");
    assert_eq!(linux_pressure_state(70, None, defaults()).0, "YELLOW");
    assert_eq!(linux_pressure_state(85, None, defaults()).0, "RED");
    // Sustained full stalls escalate an otherwise idle machine.
    assert_eq!(linux_pressure_state(10, Some(2.0), defaults()).0, "YELLOW");
    assert_eq!(linux_pressure_state(10, Some(6.0), defaults()).0, "RED");
}

#[test]
fn parses_memory_pressure_stall_average() {
    let text = "some avg10=0.00 avg60=0.00 avg300=0.00 total=18031870\n\
                full avg10=2.50 avg60=1.00 avg300=0.20 total=17677407\n";
    assert_eq!(parse_memory_pressure_stall(text), Some(2.5));
    assert_eq!(parse_memory_pressure_stall(""), None);
}

#[test]
fn linux_temperature_does_not_invent_a_throttling_signal() {
    assert_eq!(linux_thermal_label(Some(95)), "95°C measured");
    assert_eq!(linux_thermal_label(Some(85)), "85°C measured");
    assert_eq!(linux_thermal_label(Some(40)), "40°C measured");
    assert_eq!(linux_thermal_label(None), "unavailable");
}

#[test]
fn compression_summary_keeps_footprint_and_ratio_visible() {
    let sample = Sample {
        vm_available: true,
        compressor: 512 * 1024 * 1024,
        compressed_logical: 4 * 1024 * 1024 * 1024,
        ..Sample::default()
    };
    assert_eq!(compressed_memory_label(&sample), "512.0 MiB · 8.0×");
}

#[test]
fn chart_colors_follow_load_thresholds() {
    assert_eq!(ChartMetric::Memory.tone(69, defaults()), Tone::Green);
    assert_eq!(ChartMetric::Memory.tone(70, defaults()), Tone::Yellow);
    assert_eq!(ChartMetric::Memory.tone(85, defaults()), Tone::Red);
    assert_eq!(ChartMetric::Swap.tone(0, defaults()), Tone::Green);
    assert_eq!(
        ChartMetric::Swap.tone(1024 * 1024, defaults()),
        Tone::Yellow
    );
    assert_eq!(
        ChartMetric::Swap.tone(16 * 1024 * 1024, defaults()),
        Tone::Red
    );
    assert_eq!(
        crate::history::paging_tone(32 * MIB, "SWAP THRASHING", defaults()),
        Tone::Red
    );
}

#[test]
fn user_facing_load_labels_describe_conditions_not_colors() {
    let sample = Sample {
        pressure: "GREEN".into(),
        pressure_meaning: "normal".into(),
        ..Sample::default()
    };
    assert_eq!(pressure_state_label(&sample), "normal");

    let sample = Sample {
        pressure: "YELLOW".into(),
        pressure_meaning: "warning".into(),
        ..Sample::default()
    };
    assert_eq!(pressure_state_label(&sample), "watch");

    let sample = Sample {
        pressure: "RED".into(),
        pressure_meaning: "critical".into(),
        ..Sample::default()
    };
    assert_eq!(pressure_state_label(&sample), "critical");
    assert_eq!(gpu_load_label(Some(40), defaults()), "within target");
    assert_eq!(gpu_load_label(Some(80), defaults()), "busy");
    assert_eq!(gpu_load_label(Some(95), defaults()), "saturated");
    assert_eq!(gpu_load_label(None, defaults()), "unavailable");
}

#[test]
fn chart_stats_use_normalized_values_and_keep_the_window_label_honest() {
    let history = VecDeque::from([
        ChartPoint::new(Some(0), Tone::Green),
        ChartPoint::new(Some(16 * 1024 * 1024), Tone::Red),
        ChartPoint::new(None, Tone::Muted),
    ]);
    // Paging stats stay in raw bytes/s so they match the live label.
    assert_eq!(
        chart_stats(&history, ChartMetric::Swap),
        (Some(8 * 1024 * 1024), Some(16 * 1024 * 1024))
    );
    assert_eq!(
        chart_stat_label(ChartMetric::Swap, Some(57 * 1024 + 640)),
        "57.6 KiB/s"
    );
    assert_eq!(
        chart_window_label(history.len(), Duration::from_secs(60)),
        "window 3m"
    );
}

#[test]
fn overview_renders_the_first_glance_cards_at_reference_size() {
    let rendered = render_app(&test_app(0), 180, 50);
    for label in [
        "queue",
        "memory",
        "paging / I/O",
        "GPU",
        "cache",
        "generation",
        "prefill",
        "cache",
        "queue",
        "pause",
    ] {
        assert!(rendered.contains(label), "missing rendered label: {label}");
    }
    assert!(!rendered.contains("/ local LLM performance"));
    assert!(!rendered.contains("LLM READY  HEALTHY"));
}

pub(super) fn nvidia_app(count: usize) -> App {
    let mut app = test_app(0);
    let csv = (0..count)
        .map(|i| {
            format!(
                "{i}, GPU-fixture-{i}, NVIDIA RTX 4090 #{i}, {}, {}, 24564, {}",
                [97, 62, 0, 85][i % 4],
                [22100, 16800, 1024, 19700][i % 4],
                [76, 63, 35, 71][i % 4]
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    app.collector.current = Sample {
        updated: "12:30:00".into(),
        gpus: gpu::parse(&csv),
        gpu_util: Some(97),
        total_memory: 128 * 1024 * MIB,
        availability: Some(62),
        pressure: "GREEN".into(),
        pressure_meaning: "normal".into(),
        pressure_tone: Tone::Green,
        vm_available: true,
        swap_available: true,
        rate_ready: true,
        thermal: "no warning".into(),
        llm_provider: "llama.cpp".into(),
        llm_model: "local-model-70B".into(),
        llm_status: "generating".into(),
        llm_source: TelemetrySource::Live,
        llm_observed_at: Some(SystemTime::now()),
        llm_count: 1,
        llm_generation_tps: Some(42.5),
        llm_generation_tps_live: true,
        llm_prefill_tps: Some(980.0),
        llm_active_requests: Some(1),
        llm_waiting_requests: Some(0),
        llm_rss: 38 * 1024 * MIB,
        llm_cpu: 128.0,
        ..Sample::default()
    };
    classify(&mut app.collector.current, None, defaults());
    for i in 0..80 {
        app.collector
            .generation_history
            .push_back(ChartPoint::new(Some(350 + i % 9 * 10), Tone::Cyan));
        app.collector
            .load_history
            .push_back(ChartPoint::new(Some(38), Tone::Green));
        app.collector
            .gpu_history
            .push_back(ChartPoint::new(Some(88 + i % 10), Tone::Blue));
        app.collector
            .swap_history
            .push_back(ChartPoint::new(Some(0), Tone::Green));
        app.collector
            .operator_history
            .observe(&app.collector.current, 100);
    }
    *app.collector.generation_history.back_mut().unwrap() = ChartPoint::new(Some(425), Tone::Cyan);
    app
}

#[test]
fn nvidia_overview_keeps_primary_values_and_every_card_reachable() {
    for count in [1, 2, 4, 8, 16] {
        for (width, height) in [(80, 24), (100, 32), (120, 40), (180, 50)] {
            let mut app = nvidia_app(count);
            assert_eq!(app.collector.current.health, Some(100));
            assert_eq!(llm_status_tone("generating"), Tone::Cyan);
            let screen = render_nvidia_overview(&app, width, height);
            for label in [
                "GPU DEVICES",
                "UTILIZATION",
                "VRAM USED / TOTAL",
                "TEMP",
                "97%",
                "76°C",
                "21.6/24.0 GiB",
            ] {
                assert!(
                    screen.contains(label),
                    "missing {label} at {width}x{height}, {count} cards\n{screen}"
                );
            }
            assert!(screen.contains("queue"));
            assert!(!screen.contains("DIAGNOSIS"));
            assert!(screen.contains("42.5"));
            assert!(screen.contains("prompt load"));
            for _ in 1..count {
                app.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
            }
            assert_eq!(app.gpu_selected, count - 1);
            let last = render_nvidia_overview(&app, width, height);
            assert!(
                last.contains(&format!("▸ {}", count - 1)),
                "last GPU inaccessible at {width}x{height}\n{last}"
            );
            assert_eq!(app.request_scroll, 0);
            if count > 1 {
                app.handle_key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE));
                assert_eq!(app.gpu_selected, count - 2);
            }
        }
    }
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
fn gpu_journal_uses_load_words_and_measured_recovery_colors() {
    let transition = |previous: u8, current: u8, thresholds: Thresholds| {
        let mut band = None;
        gpu_journal_transition(&mut band, Some(previous), thresholds);
        gpu_journal_transition(&mut band, Some(current), thresholds)
    };
    for (previous, current, label, tone) in [
        (20, 95, "GPU load saturated · 95% busy", Tone::Red),
        (98, 78, "GPU load eased · 78% busy", Tone::Yellow),
        (98, 0, "GPU idle · 0% busy", Tone::Green),
        (20, 85, "busy burst started · 85% busy", Tone::Yellow),
        (85, 40, "GPU load eased · 40% busy", Tone::Green),
    ] {
        let (summary, actual) = transition(previous, current, defaults()).unwrap();
        assert_eq!(summary, label);
        assert_eq!(actual, tone);
        assert!(!summary.contains("critical"));
    }
    let mut band = None;
    assert!(gpu_journal_transition(&mut band, None, defaults()).is_none());
    assert!(gpu_journal_transition(&mut band, Some(0), defaults()).is_none());
    assert!(transition(75, 74, defaults()).is_none());
    let thresholds = Thresholds {
        gpu_critical_load: 95,
        ..defaults()
    };
    assert!(transition(89, 94, thresholds).is_none());
    assert!(transition(94, 95, thresholds)
        .unwrap()
        .0
        .contains("saturated"));
    // A configured exit level holds the busy band down to it.
    let thresholds = Thresholds {
        gpu_warn_exit: 50,
        ..defaults()
    };
    assert!(transition(84, 55, thresholds).is_none());
}

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
fn tab_switches_views_and_arrows_stay_contextual() {
    let mut app = test_app(0);
    render_app(&app, 180, 50);
    // Host resources sit above prompt load; throughput sits below it.
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.charts.focused, Chart::Cache);
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(app.charts.focused, Chart::Memory);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.charts.focused, Chart::Cache);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.charts.focused, Chart::Paging);
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.charts.focused, Chart::Prompt);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.tab, 0);
    assert_eq!(app.charts.focused, Chart::Generation);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    for expected in [1, 2, 3, 0] {
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.tab, expected);
        assert!(!app.charts.expanded);
    }
    for expected in [3, 2, 1, 0] {
        app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(app.tab, expected);
    }
    app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.tab, 2);
    assert!(!app.top_filtering);
    assert_eq!(app.top_filter, "x");
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
fn chart_controls_focus_zoom_and_expand_without_changing_sampling() {
    let mut app = test_app(0);
    render_app(&app, 180, 50);
    let original_interval = app.interval;
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.charts.focused, Chart::Generation);
    app.handle_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));
    assert_eq!(app.charts.zoom(Chart::Generation), 2);
    assert_eq!(app.charts.zoom(Chart::Prompt), 1);
    assert_eq!(app.interval, original_interval);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.charts.focused, Chart::Prefill);
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(app.charts.focused, Chart::Generation);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let screen = render_app(&app, 80, 24);
    assert!(screen.contains("generation"));
    assert!(screen.contains("Enter restore"));
    assert!(!screen.contains("LLM OPERATIONS"));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.charts.expanded);
    assert!(!app.quit);
    assert_eq!(app.charts.zoom(Chart::Generation), 2);
    app.handle_key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::NONE));
    assert_eq!(app.charts.zoom(Chart::Generation), 1);
    app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
    assert_eq!(app.tab, 1);
}

#[test]
fn mouse_targets_visible_charts_and_resize_replaces_hit_regions() {
    let mut app = test_app(0);
    render_app(&app, 180, 50);
    let area = app
        .charts
        .regions
        .borrow()
        .iter()
        .find(|(chart, _)| *chart == Chart::Queue)
        .unwrap()
        .1;
    let mouse = |kind| MouseEvent {
        kind,
        column: area.x + 1,
        row: area.y + 1,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left)));
    assert_eq!(app.charts.focused, Chart::Queue);
    for _ in 0..10 {
        app.handle_mouse(mouse(MouseEventKind::ScrollUp));
    }
    assert_eq!(app.charts.zoom(Chart::Queue), 8);
    assert_eq!(app.charts.zoom(Chart::Gpu), 8);
    for _ in 0..10 {
        app.handle_mouse(mouse(MouseEventKind::ScrollDown));
    }
    assert_eq!(app.charts.zoom(Chart::Queue), 1);
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right)));
    assert!(app.charts.expanded);
    app.help = true;
    app.handle_mouse(mouse(MouseEventKind::ScrollUp));
    assert_eq!(app.charts.zoom(Chart::Queue), 1);
    app.help = false;
    app.charts.expanded = false;
    render_app(&app, 80, 24);
    assert!(app
        .charts
        .regions
        .borrow()
        .iter()
        .all(|(_, area)| area.right() <= 80 && area.bottom() <= 24));
    render_app(&app, 60, 20);
    assert!(app.charts.regions.borrow().is_empty());
}

#[test]
fn paging_alert_rings_once_per_episode_and_rearms() {
    let (mut app, views) = test_app_with_sender(0);
    app.collector.current.impact = "LLM READY".into();

    views
        .send(view_with_impact("HEAVY PAGING", "00:52:54"))
        .expect("test channel should accept views");
    app.tick();
    let alert = app.alert.as_ref().expect("alert should raise");
    assert_eq!(alert.state, "HEAVY PAGING");
    assert_eq!(alert.summary, "swap 0 B/s / growth 0 B/s");
    assert_eq!(alert.time, "00:52:54");
    assert_eq!(app.alert_bells, 1);

    // Staying aggressive must not re-ring; escalation refreshes the
    // banner without raising a second alert.
    views
        .send(view_with_impact("SWAP THRASHING", "00:52:55"))
        .expect("test channel should accept views");
    app.tick();
    let alert = app.alert.as_ref().expect("alert persists while paging");
    assert_eq!(alert.state, "SWAP THRASHING");
    assert_eq!(alert.time, "00:52:55");
    assert_eq!(app.alert_bells, 1);

    // Recovery retires the alert and re-arms it for the next episode.
    views
        .send(view_with_impact("LLM READY", "00:52:56"))
        .expect("test channel should accept views");
    app.tick();
    assert!(app.alert.is_none());
    views
        .send(view_with_impact("HEAVY PAGING", "00:53:10"))
        .expect("test channel should accept views");
    app.tick();
    assert_eq!(app.alert_bells, 2);
    assert_eq!(app.alert.as_ref().expect("rearmed").time, "00:53:10");
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
fn overview_uses_semantic_state_labels_instead_of_color_names() {
    let mut app = test_app(0);
    app.collector.current.pressure = "GREEN".into();
    app.collector.current.pressure_meaning = "normal".into();
    app.collector.current.pressure_tone = Tone::Green;
    app.collector.current.availability = Some(50);
    let rendered = render_app(&app, 180, 50);

    assert!(rendered.contains("PRESSURE normal"));
    assert!(!rendered.contains("GREEN"));
    assert!(!rendered.contains("YELLOW"));
    assert!(!rendered.contains("RED"));
}

#[test]
fn overview_renders_stepped_traces_instead_of_braille_points() {
    let mut app = test_app(0);
    app.collector.load_history = VecDeque::from([
        ChartPoint::new(Some(10), Tone::Green),
        ChartPoint::new(Some(100), Tone::Red),
    ]);
    let rendered = render_app(&app, 180, 50);

    assert!(rendered.contains('━'));
    assert!(!rendered.contains('⠁'));
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
fn overview_omits_process_memory_chart_even_when_available() {
    let mut app = test_app(0);
    app.collector.current.process_memory = Some(process_memory::Reading {
        pid: 37966,
        started: 1,
        resident: 16 * 1024 * MIB,
        footprint: 17 * 1024 * MIB,
        peak: 29 * 1024 * MIB,
        disk_read: 0,
        disk_written: 0,
        at: Instant::now(),
    });
    let screen = render_app(&app, 180, 50);
    assert!(!screen.contains("process memory"));
    assert!(!screen.contains("OS footprint"));
}

#[test]
fn requests_dashboard_renders_counts_history_and_empty_state() {
    let mut app = test_app(0);
    app.collector.current.llm_provider = "oMLX".into();
    app.collector.current.llm_model = "test-model".into();
    let empty = render_app(&app, 80, 24);
    assert!(empty.contains("no requests yet"), "{empty}");
    assert!(!empty.contains("0–1 tokens"));
    assert!(
        app.charts
            .regions
            .borrow()
            .iter()
            .find(|(chart, _)| *chart == Chart::Prompt)
            .unwrap()
            .1
            .height
            == 6,
        "no requests: one line, plus what cache and queue need"
    );
    for (i, prompt) in [12000, 20000, 32768].into_iter().enumerate() {
        app.collector
            .request_history
            .observe(&[domain::RequestUsage {
                provider: "oMLX".into(),
                model: "test-model".into(),
                id: format!("req-{i}"),
                prompt,
                cached: Some(prompt / 2),
                output: Some(40),
                completed: true,
                queued: false,
                prefilled: None,
                idle_for: None,
                ttft_ms: None,
                output_tps: None,
                observed_at: Some(SystemTime::now()),
            }]);
    }
    for (width, height) in [(80, 24), (100, 40), (180, 50)] {
        let screen = render_app(&app, width, height);
        for label in ["prompt load", "32,768", "CACHE", "12.0k", "20.0k", "32.8k▲"] {
            assert!(
                screen.contains(label),
                "missing {label} at {width}x{height}"
            );
        }
        assert!(screen.contains("vs previous"), "{width}x{height}\n{screen}");
    }
    app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    assert_eq!(app.request_scroll, 2);
    let older = render_app(&app, 80, 24);
    assert!(older.contains("12,000"));
    assert!(!older.contains("20,000"));
    app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
    assert_eq!(app.request_scroll, 0);
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.tab, 1);
    assert_eq!(app.charts.focused, Chart::Prompt);
    app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
    assert_eq!(app.tab, 1);
    app.handle_key(KeyEvent::new(KeyCode::Char('5'), KeyModifiers::NONE));
    assert_eq!(app.tab, 1);
    assert!(!render_app(&app, 100, 40).contains("4 REQ"));
}

#[test]
fn overview_sizes_histories_by_importance_and_keeps_idle_geometry_stable() {
    let mut app = test_app(0);
    for (width, height, rates_rows, request_rows) in [
        (80, 24, 4, 6),
        (100, 40, 6, 6),
        (170, 42, 7, 7),
        (180, 50, 8, 8),
    ] {
        app.collector.request_history = request_history::History::default();
        render_app(&app, width, height);
        let empty_regions = app.charts.regions.borrow().clone();
        let region = |chart| empty_regions.iter().find(|(id, _)| *id == chart).unwrap().1;
        // No requests yet: the row scales with the terminal like the Journal,
        // instead of pinning six rows on every screen size.
        assert_eq!(region(Chart::Prompt).height, request_rows);
        assert_eq!(
            region(Chart::Generation).height,
            rates_rows,
            "throughput scales with the terminal at {width}x{height}"
        );
        assert!(
            region(Chart::Memory).height > region(Chart::Generation).height,
            "host resources outrank throughput at {width}x{height}"
        );
        assert_eq!(region(Chart::Generation).y, region(Chart::Prefill).y);
        assert_eq!(
            region(Chart::Generation).height,
            region(Chart::Prefill).height
        );
        assert!(region(Chart::Generation).right() <= region(Chart::Prefill).x);
        assert!(region(Chart::Prompt).bottom() <= region(Chart::Generation).y);
        assert_eq!(region(Chart::Prompt).y, region(Chart::Compression).y);
        assert_eq!(region(Chart::Memory).y, region(Chart::Cache).y);
        assert_eq!(
            region(Chart::Queue).bottom(),
            region(Chart::Prompt).bottom()
        );
        // One column grid: every row's edges line up.
        assert_eq!(
            region(Chart::Memory).right(),
            region(Chart::Generation).right()
        );
        assert_eq!(region(Chart::Cache).right(), region(Chart::Prompt).right());
        assert_eq!(region(Chart::Paging).x, region(Chart::Compression).x);
        assert_eq!(region(Chart::Paging).x, region(Chart::Gpu).x);
        for chart in [Chart::Memory, Chart::Cache, Chart::Paging] {
            assert_eq!(region(chart).y, region(Chart::Memory).y);
            assert!(region(chart).bottom() <= region(Chart::Prompt).y);
            assert!(region(chart).height >= 6);
        }
        assert_eq!(region(Chart::Generation).y, region(Chart::Gpu).y);
        assert!(
            region(Chart::Generation)
                .width
                .abs_diff(region(Chart::Prefill).width)
                <= 1
        );
        if height >= 40 {
            assert!(region(Chart::Memory).height >= 12);
            // The rates row never drops below its compact floor.
            assert!(region(Chart::Prefill).height >= 4);
        }
        // Requests arriving give prompt load its rows back; the throughput
        // row and every column edge stay where they were.
        populate_dashboard_fixture(&mut app);
        render_app(&app, width, height);
        let live = app.charts.regions.borrow().clone();
        let live_region = |chart| live.iter().find(|(id, _)| *id == chart).unwrap().1;
        assert!(live_region(Chart::Prompt).height >= 7);
        for chart in [Chart::Generation, Chart::Prefill, Chart::Gpu] {
            assert_eq!(live_region(chart), region(chart), "{width}x{height}");
        }
        assert_eq!(
            live.iter()
                .map(|(chart, area)| (*chart, area.x, area.width))
                .collect::<Vec<_>>(),
            empty_regions
                .iter()
                .map(|(chart, area)| (*chart, area.x, area.width))
                .collect::<Vec<_>>(),
            "request arrival must not rearrange the dashboard at {width}x{height}"
        );
        app.collector.current.pressure = "RED".into();
        app.collector.current.swap_in = 32 * MIB;
        app.collector.current.llm_waiting_requests = Some(12);
        render_app(&app, width, height);
        assert_eq!(
            *app.charts.regions.borrow(),
            live,
            "transient counters must not move the charts at {width}x{height}"
        );
    }
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
fn overview_uses_a_compact_operational_strip_on_medium_terminals() {
    let rendered = render_app(&test_app(0), 100, 40);
    for label in ["queue", "generation", "GPU", "memory", "paging"] {
        assert!(rendered.contains(label), "missing compact label: {label}");
    }
    assert!(!rendered.contains("Terminal too small"));
}

#[test]
fn overview_remains_useful_at_a_classic_eighty_column_size() {
    let rendered = render_app(&test_app(0), 80, 24);
    for label in ["generation", "GPU", "memory", "paging"] {
        assert!(rendered.contains(label), "missing 80-column label: {label}");
    }
    assert!(!rendered.contains("This dashboard needs"));
}

#[test]
fn process_table_adapts_to_medium_terminals() {
    let rendered = render_app(&test_app(1), 100, 40);
    for label in ["LLM PROCESSES", "SELECTED PROCESS", "PAGEIN/s", "OS STATE"] {
        assert!(rendered.contains(label), "missing process label: {label}");
    }
    assert!(!rendered.contains("MEM%"));
}

#[test]
fn process_view_remains_scannable_at_eighty_columns() {
    let rendered = render_app(&test_app(1), 80, 24);
    for label in [
        "SELECTED PROCESS",
        "No local LLM processes",
        "LLM PROCESSES",
        "PROCESS",
    ] {
        assert!(
            rendered.contains(label),
            "missing narrow process label: {label}"
        );
    }
    assert!(!rendered.contains("This dashboard needs"));
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
fn llm_top_sort_cycles_in_a_predictable_order() {
    assert_eq!(TopSort::Rss.next(), TopSort::Cpu);
    assert_eq!(TopSort::Cpu.next(), TopSort::Pid);
    assert_eq!(TopSort::Pid.next(), TopSort::Name);
    assert_eq!(TopSort::Name.next(), TopSort::Rss);
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
fn splash_engine_under_another_runtime_never_becomes_the_provider() {
    let omlx = "100 2097152 3.0 1.0 S 0 omlx-server /usr/local/bin/omlx-server --port 8000";
    let engine = "200 20971520 90.0 30.0 R 0 splash \
                  /opt/homebrew/Cellar/splash/1.3.0/libexec/engine/splash --port 53122";
    // oMLX 0.7.1 serves models through a Splash child on a private port;
    // either ps order selects oMLX, and both count as LLM processes.
    for rows in [format!("{omlx}\n{engine}"), format!("{engine}\n{omlx}")] {
        let snapshot = parse_processes(&rows);
        assert_eq!(snapshot.provider.as_deref(), Some("oMLX"), "{rows}");
        assert_eq!(snapshot.llm_count, 2);
        assert_eq!(snapshot.top_llm.map(|process| process.pid), Some(200));
        assert_eq!(snapshot.hosted_splash_cpu, Some(90.0));
    }
    // Splash on its own is still Splash, and hosts nothing.
    let alone = parse_processes(engine);
    assert_eq!(alone.provider.as_deref(), Some("Splash"));
    assert_eq!(alone.hosted_splash_cpu, None);
    assert_eq!(parse_processes(omlx).hosted_splash_cpu, None);
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

fn is_llm_process(name: &str, command: &str) -> bool {
    process_provider(name, command).is_some()
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
fn bars_fill_whole_cells_then_an_eighth_block_and_never_vanish() {
    assert_eq!(bar_cells(0, 4), Vec::<char>::new());
    assert_eq!(bar_cells(100, 4), ['█'; 4]);
    assert_eq!(bar_cells(50, 4), ['█', '█']);
    // 60% of 4 rows is 19.2 eighths: two full cells and a three-eighth top.
    assert_eq!(bar_cells(60, 4), ['█', '█', '▃']);
    // A measured rate too small for an eighth still shows.
    assert_eq!(bar_cells(1, 4), ['▁']);
    assert_eq!(bar_cells(250, 2), ['█', '█'], "clamped to the plot");
}

#[test]
fn throughput_axes_follow_visible_workload_and_zoom() {
    let history = VecDeque::from([
        ChartPoint::new(Some(20_000), Tone::Cyan),
        ChartPoint::new(Some(350), Tone::Green),
        ChartPoint::new(Some(355), Tone::Yellow),
        ChartPoint::new(Some(360), Tone::Red),
    ]);
    // Rate bars stand on zero; the top follows the visible peak and zoom.
    for metric in [ChartMetric::Generation, ChartMetric::Prefill] {
        let wide = chart_scale(&history, metric, 4);
        let zoomed = chart_scale(&history, metric, 3);
        assert_eq!((wide.0, zoomed.0), (0, 0));
        assert!(wide.1 >= 20_000);
        assert!(zoomed.1 >= 360 && zoomed.1 <= 450, "{zoomed:?}");
        assert_eq!(chart_display_value(metric, 0, zoomed), 0);
        assert_eq!(chart_display_value(metric, zoomed.1, zoomed), 100);
    }
    assert_eq!(chart_stat_label(ChartMetric::Generation, Some(355)), "35.5");
    let zoomed = chart_scale(&history, ChartMetric::Generation, 3);
    let columns = chart_columns_for_plot(&history, 3, ChartMetric::Generation, 10, zoomed);
    assert_eq!(
        columns.iter().map(|point| point.tone).collect::<Vec<_>>(),
        [Tone::Green, Tone::Yellow, Tone::Red]
    );
}

#[test]
fn inactive_rate_chart_identifies_the_other_active_phase() {
    let prefilling = Sample {
        llm_status: "prefilling".into(),
        ..Sample::default()
    };
    assert_eq!(
        chart_inactive_rate_label(ChartMetric::Generation, &prefilling),
        "prefill active"
    );
    assert_eq!(
        chart_inactive_rate_label(ChartMetric::Prefill, &prefilling),
        "active"
    );

    let generating = Sample {
        llm_status: "generating".into(),
        ..Sample::default()
    };
    assert_eq!(
        chart_inactive_rate_label(ChartMetric::Prefill, &generating),
        "decode active"
    );
}

#[test]
fn stepped_chart_uses_thin_trace_bars() {
    assert_eq!(trace_point(0, 4), Some((3, '━')));
    assert_eq!(trace_point(1, 4), Some((3, '━')));
    assert_eq!(trace_point(50, 4), Some((1, '━')));
    assert_eq!(trace_point(7, 9), Some((7, '━')));
    assert_eq!(trace_point(100, 4), Some((0, '━')));
    assert_eq!(trace_point(100, 0), None);
}

#[test]
fn percent_midpoint_trace_lines_up_with_its_tick_at_even_and_odd_heights() {
    let app = test_app(0);
    let history = VecDeque::from(vec![ChartPoint::new(Some(50), Tone::Cyan); 40]);
    for height in 6..12 {
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(40, height)).unwrap();
        terminal
            .draw(|frame| {
                app.render_indicator_chart(frame, frame.area(), "GPU", &history, ChartMetric::Gpu)
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let middle = 1 + (height - 3) / 2;
        let tick: String = (1..5).map(|x| buffer[(x, middle)].symbol()).collect();
        assert_eq!(tick.trim(), "50%");
        assert_eq!(buffer[(38, middle)].symbol(), "━");
        assert_eq!(buffer[(38, middle)].fg, CYAN);
    }
}

#[test]
fn stepped_chart_connectors_follow_the_level_being_crossed() {
    let mut cells = vec![
        vec![
            TraceCell {
                glyph: ' ',
                tone: Tone::Muted,
                fill: false,
            };
            3
        ];
        7
    ];
    trace_connector(
        &mut cells,
        1,
        0,
        6,
        TraceStyle {
            metric: ChartMetric::Gpu,
            previous_tone: Tone::Red,
            tone: Tone::Green,
            thresholds: defaults(),
            scale: (0, 100),
        },
    );
    assert_eq!(cells[0][1].glyph, '┓');
    assert_eq!(cells[0][1].tone, Tone::Red);
    assert_eq!(cells[1][1].glyph, '┃');
    assert_eq!(cells[1][1].tone, Tone::Yellow);
    assert_eq!(cells[2][1].tone, Tone::Green);
    assert_eq!(cells[6][1].glyph, '┗');
    assert_eq!(cells[6][1].tone, Tone::Green);

    trace_connector(
        &mut cells,
        2,
        6,
        0,
        TraceStyle {
            metric: ChartMetric::Gpu,
            previous_tone: Tone::Green,
            tone: Tone::Yellow,
            thresholds: defaults(),
            scale: (0, 100),
        },
    );
    assert_eq!(cells[0][2].glyph, '┏');
    assert_eq!(cells[0][2].tone, Tone::Yellow);
    assert_eq!(cells[1][2].tone, Tone::Yellow);
    assert_eq!(cells[2][2].tone, Tone::Green);
    assert_eq!(cells[6][2].glyph, '┛');
    assert_eq!(cells[6][2].tone, Tone::Green);
}

#[test]
fn chart_columns_keep_samples_in_a_scrolling_ring_buffer() {
    let history = VecDeque::from([
        ChartPoint::new(Some(10), Tone::Green),
        ChartPoint::new(Some(20), Tone::Yellow),
    ]);
    let columns = chart_columns(&history, 6);
    assert_eq!(
        columns.iter().map(|point| point.value).collect::<Vec<_>>(),
        vec![None, None, None, None, Some(10), Some(20)]
    );
    assert_eq!(columns[4].tone, Tone::Green);
    assert_eq!(columns[5].tone, Tone::Yellow);
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
fn chart_columns_keep_gaps_disconnected_when_scrolled() {
    let history = VecDeque::from([
        ChartPoint::new(Some(10), Tone::Green),
        ChartPoint::new(None, Tone::Muted),
        ChartPoint::new(Some(30), Tone::Green),
    ]);
    let columns = chart_columns(&history, 4);
    assert_eq!(
        columns.iter().map(|point| point.value).collect::<Vec<_>>(),
        vec![None, Some(10), None, Some(30)]
    );
    assert!(columns[2].break_before);
    assert!(!columns[3].break_before);
}

#[test]
fn chart_columns_render_all_missing_data_without_dividing_by_zero() {
    let history = VecDeque::from([
        ChartPoint::new(None, Tone::Muted),
        ChartPoint::new(None, Tone::Muted),
        ChartPoint::new(None, Tone::Muted),
        ChartPoint::new(None, Tone::Muted),
        ChartPoint::new(None, Tone::Muted),
    ]);
    let columns = chart_columns(&history, 3);
    assert_eq!(columns.len(), 3);
    assert!(columns.iter().all(|point| point.value.is_none()));
}

#[test]
fn bar_chart_keeps_each_sample_tone_independent() {
    let history = VecDeque::from([
        ChartPoint::new(Some(69), ChartMetric::Memory.tone(69, defaults())),
        ChartPoint::new(Some(85), ChartMetric::Memory.tone(85, defaults())),
    ]);
    assert_eq!(history[0].tone, Tone::Green);
    assert_eq!(history[1].tone, Tone::Red);
}

#[test]
fn chart_smoothing_uses_dynamic_visual_resolution() {
    let history = VecDeque::from([
        ChartPoint::new(Some(100), Tone::Red),
        ChartPoint::new(Some(99), Tone::Red),
        ChartPoint::new(Some(98), Tone::Red),
        ChartPoint::new(Some(80), Tone::Yellow),
    ]);
    assert_eq!(
        chart_plot_values(&history, ChartMetric::Gpu, 10),
        vec![Some(100), Some(100), Some(100), Some(80)]
    );
    assert_eq!(
        chart_plot_values(&history, ChartMetric::Gpu, 20),
        vec![Some(100), Some(100), Some(98), Some(80)]
    );

    let generation_history = VecDeque::from([
        ChartPoint::new(Some(300), Tone::Cyan),
        ChartPoint::new(Some(299), Tone::Cyan),
        ChartPoint::new(Some(250), Tone::Cyan),
    ]);
    assert_eq!(
        chart_plot_values(&generation_history, ChartMetric::Generation, 10),
        vec![Some(300), Some(300), Some(250)]
    );

    let burst_history = VecDeque::from([
        ChartPoint::new(Some(0), Tone::Green),
        ChartPoint::new(Some(100), Tone::Red),
        ChartPoint::new(Some(0), Tone::Green),
        ChartPoint::new(Some(100), Tone::Red),
    ]);
    assert_eq!(
        chart_plot_values(&burst_history, ChartMetric::Gpu, 10),
        vec![Some(0), Some(100), Some(0), Some(100)]
    );
}

#[test]
fn chart_smoothing_resets_after_missing_data() {
    let history = VecDeque::from([
        ChartPoint::new(Some(100), Tone::Red),
        ChartPoint::new(None, Tone::Muted),
        ChartPoint::new(Some(99), Tone::Red),
    ]);

    assert_eq!(
        chart_plot_values(&history, ChartMetric::Gpu, 10),
        vec![Some(100), None, Some(99)]
    );
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
fn severity_thresholds_have_a_recovery_band() {
    assert!(!threshold_with_hysteresis(
        69,
        false,
        70,
        defaults().gpu_warn_exit
    ));
    assert!(threshold_with_hysteresis(
        75,
        false,
        70,
        defaults().gpu_warn_exit
    ));
    assert!(threshold_with_hysteresis(
        71,
        true,
        80,
        defaults().gpu_warn_exit
    ));
    assert!(!threshold_with_hysteresis(
        69,
        true,
        80,
        defaults().gpu_warn_exit
    ));
}

#[test]
fn journal_filters_use_structured_event_kinds() {
    assert!(JournalFilter::Llm.matches(EventKind::Llm));
    assert!(JournalFilter::Llm.matches(EventKind::Queue));
    assert!(!JournalFilter::Llm.matches(EventKind::Gpu));
    assert!(JournalFilter::Paging.matches(EventKind::Paging));
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
fn file_paging_counts_against_the_system_only_while_memory_is_tight() {
    let mib = 1024 * 1024;
    let roomy = Sample {
        rate_ready: true,
        total_memory: 36 * 1024 * mib,
        availability: Some(40),
        pressure: "GREEN".into(),
        vm_available: true,
        swap_available: true,
        thermal: "no warning".into(),
        llm_count: 1,
        // A model loading from SSD with swap off: page-ins, no swap.
        page_in: 900 * mib,
        page_out: mib,
        ..Sample::default()
    };
    assert!(!roomy.memory_tight());
    assert_eq!(roomy.paging_churn(), 0);
    assert_eq!(roomy.disk_paging(), 901 * mib);
    let mut sample = roomy.clone();
    classify(&mut sample, None, defaults());
    assert_eq!(
        sample.impact, "LLM READY",
        "a model load is not degradation"
    );

    // Little memory free while macOS still says normal pressure: the same
    // page-ins are pages evicted and read straight back.
    let mut full = Sample {
        availability: Some(TIGHT_FREE_PERCENT),
        ..roomy.clone()
    };
    assert!(full.memory_tight());
    assert_eq!(full.paging_in(), 900 * mib);
    assert_eq!(full.paging_churn(), 901 * mib);
    classify(&mut full, None, defaults());
    assert_eq!(full.impact, "HEAVY PAGING");
    assert_eq!(full.impact_tone, Tone::Red);
    assert_eq!(full.guidance_badge, "ACT NOW");
    assert_eq!(
        full.guidance_cause,
        "RAM full — reading evicted pages back from disk at 900.0 MiB/s."
    );
    assert!(signal_summary(&full).starts_with("page-in 900.0 MiB/s"));

    // Lighter file paging under pressure is active paging, not yet heavy.
    let mut pressured = Sample {
        pressure: "YELLOW".into(),
        page_in: 8 * mib,
        page_out: 0,
        ..roomy.clone()
    };
    classify(&mut pressured, None, defaults());
    assert_eq!(pressured.impact, "MEMORY STRESS");
    pressured.pressure = "GREEN".into();
    pressured.availability = Some(5);
    classify(&mut pressured, None, defaults());
    assert_eq!(pressured.impact, "PAGING ACTIVE");
    assert_eq!(
        pressured.guidance_cause,
        "Paging active — in 8.0 MiB/s + out 0 B/s."
    );

    // Swap counts whatever the memory state, as before.
    let mut swapping = Sample {
        swap_in: 8 * mib,
        page_in: 0,
        page_out: 0,
        ..roomy
    };
    classify(&mut swapping, None, defaults());
    assert_eq!(swapping.impact, "PAGING ACTIVE");
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
fn omlx_prefill_initial_speed_is_missing_until_a_chunk_rate_exists() {
    let health = json!({"status":"healthy", "default_model":"model"});
    let mut stats = json!({
        "avg_prefill_tps":120.6,
        "active_models":{"models":[{
            "id":"model", "active_requests":1, "waiting_requests":0,
            "prefilling":[{"processed":0,"total":2947,"speed":0,"prompt_tokens":2947}]
        }]}
    });
    let telemetry = parse_omlx_telemetry(&health, Some(&stats));
    assert_eq!(telemetry.status.as_deref(), Some("prefilling"));
    assert_eq!(telemetry.prefill_tps, Some(120.6));
    assert!(!telemetry.prefill_tps_live);
    assert_eq!(
        chart_rate_value(
            &Sample {
                llm_prefill_tps: telemetry.prefill_tps,
                llm_prefill_tps_live: telemetry.prefill_tps_live,
                ..Sample::default()
            },
            ChartMetric::Prefill
        ),
        None
    );

    stats["active_models"]["models"][0]["prefilling"][0]["processed"] = json!(2048);
    stats["active_models"]["models"][0]["prefilling"][0]["speed"] = json!(118.5);
    let telemetry = parse_omlx_telemetry(&health, Some(&stats));
    assert!(telemetry.prefill_tps_live);
    assert_eq!(telemetry.prefill_tps, Some(118.5));

    // The same placeholder appears in distributed per-request progress.
    let model = json!({"cluster":{"live":{"metrics":{"active_request_metrics":[{
        "request_id":"r1", "status":"running",
        "prefill_progress":{"active":true,"speed":0}
    }]}}}});
    let requests = providers::omlx_model_requests(&model);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].phase, providers::OmlxPhase::Prefilling);
    assert_eq!(requests[0].rate, None);
}

#[test]
fn rate_charts_skip_aggregate_and_log_fallbacks() {
    let sample = Sample {
        llm_source: TelemetrySource::Live,
        llm_status: "prefilling".into(),
        llm_generation_tps: Some(23.5),
        llm_prefill_tps: Some(14.1),
        ..Sample::default()
    };
    assert_eq!(chart_rate_value(&sample, ChartMetric::Generation), None);
    assert_eq!(chart_rate_value(&sample, ChartMetric::Prefill), None);

    let active = Sample {
        llm_source: TelemetrySource::Live,
        llm_generation_tps: Some(23.5),
        llm_generation_tps_live: true,
        ..Sample::default()
    };
    assert_eq!(
        chart_rate_value(&active, ChartMetric::Generation),
        Some(235)
    );

    let log_sample = Sample {
        llm_source: TelemetrySource::Log,
        llm_generation_tps: Some(23.5),
        ..Sample::default()
    };
    assert_eq!(chart_rate_value(&log_sample, ChartMetric::Generation), None);
}

#[test]
fn cache_chart_uses_interval_counter_deltas() {
    let first = LlmTelemetry {
        total_prompt_tokens: Some(100),
        total_cached_tokens: Some(20),
        ..LlmTelemetry::default()
    };
    let second = LlmTelemetry {
        total_prompt_tokens: Some(150),
        total_cached_tokens: Some(50),
        ..LlmTelemetry::default()
    };
    let mut previous = None;

    assert_eq!(
        cache_interval_efficiency(&mut previous, Some(&first), false),
        None
    );
    assert_eq!(
        cache_interval_efficiency(&mut previous, Some(&second), false),
        Some(60.0)
    );
    assert_eq!(
        cache_interval_efficiency(&mut previous, Some(&second), false),
        None
    );
}

#[test]
fn cache_counter_resets_and_stale_intervals_leave_gaps() {
    let telemetry = |provider: &str, prompt, cached| LlmTelemetry {
        provider: Some(provider.into()),
        total_prompt_tokens: Some(prompt),
        total_cached_tokens: Some(cached),
        ..LlmTelemetry::default()
    };
    let mut previous = None;
    for (provider, prompt, cached, stale, expected) in [
        ("oMLX", 100, 50, false, None),
        ("oMLX", 200, 50, false, Some(0.0)), // a genuine cache miss
        ("oMLX", 300, 20, false, None),      // cached counter reset
        ("oMLX", 100, 10, false, None),      // server restart
        ("oMLX", 200, 160, false, None),     // inconsistent counters
        ("oMLX", 300, 210, true, None),
        ("oMLX", 400, 260, false, None), // re-establish after a stale period
        ("other", 500, 310, false, None),
        ("other", 600, 360, false, Some(50.0)),
    ] {
        assert_eq!(
            cache_interval_efficiency(
                &mut previous,
                Some(&telemetry(provider, prompt, cached)),
                stale
            ),
            expected,
        );
    }
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
fn log_rates_are_labeled_as_last_results() {
    let sample = Sample {
        llm_source: TelemetrySource::Log,
        llm_generation_tps: Some(28.5),
        ..Sample::default()
    };

    assert_eq!(llm_generation_rate_label(&sample), "LAST GEN 28.5 tok/s");
}

#[test]
fn correlation_ignores_retained_idle_rates() {
    let mut engine = CorrelationEngine::default();
    engine.observe(
        &Sample {
            llm_provider: "oMLX".into(),
            llm_model: "model".into(),
            llm_generation_tps: Some(30.0),
            ..Sample::default()
        },
        defaults(),
    );
    let insight = engine.observe(
        &Sample {
            llm_provider: "oMLX".into(),
            llm_model: "model".into(),
            llm_generation_tps: Some(25.0),
            ..Sample::default()
        },
        defaults(),
    );

    assert_eq!(insight.direction, ThroughputDirection::Unknown);
    assert_eq!(insight.cause, CorrelationCause::None);
    assert!(insight.summary.is_empty());
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
fn config_defaults_when_file_missing() {
    let config = Config::default();
    assert_eq!(config.interval, None);
    assert_eq!(config.history, None);
    assert_eq!(config.omx, None);
    assert_eq!(config.memory_warn_load, None);
    assert_eq!(config.gpu_critical_load, None);
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
fn configured_thresholds_replace_the_built_in_defaults() {
    let config: Config = serde_json::from_str(
        r#"{"memory_warn_load":60,"memory_critical_load":75,
            "gpu_warn_load":50,"gpu_critical_load":65,"gpu_warn_exit":40,
            "swap_warn_rate":2097152,"swap_critical_rate":33554432,
            "swap_warn_exit":1048576,
            "compression_warn_rate":134217728,"compression_warn_exit":67108864}"#,
    )
    .expect("valid json should parse");
    let thresholds = Thresholds::from_config(&config);

    assert_eq!(
        thresholds,
        Thresholds {
            memory_warn_load: 60,
            memory_critical_load: 75,
            gpu_warn_load: 50,
            gpu_critical_load: 65,
            gpu_warn_exit: 40,
            swap_warn_rate: 2 * MIB,
            swap_critical_rate: 32 * MIB,
            swap_warn_exit: MIB,
            compression_warn_rate: 128 * MIB,
            compression_warn_exit: 64 * MIB,
        }
    );
    assert_eq!(Thresholds::from_config(&Config::default()), defaults());
}

#[test]
fn configured_linux_memory_thresholds_move_the_pressure_band() {
    let config: Config =
        serde_json::from_str(r#"{"memory_critical_load":95}"#).expect("valid json should parse");
    let thresholds = Thresholds::from_config(&config);

    /* 85% is critical with the built-in default and only a warning once
     * the user raises the critical level to 95. */
    assert_eq!(linux_pressure_state(85, None, defaults()).1, Tone::Red);
    assert_eq!(linux_pressure_state(85, None, thresholds).1, Tone::Yellow);
    assert_eq!(linux_pressure_state(95, None, thresholds).1, Tone::Red);
    assert_eq!(linux_pressure_state(69, None, thresholds).1, Tone::Green);
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
fn configured_swap_rates_change_the_classified_impact() {
    let template = Sample {
        rate_ready: true,
        total_memory: 32 * 1024 * 1024 * 1024,
        availability: Some(50),
        pressure: "GREEN".into(),
        vm_available: true,
        swap_available: true,
        thermal: "no warning".into(),
        llm_count: 1,
        swap_in: 16 * MIB,
        swap_out: 16 * MIB,
        ..Sample::default()
    };

    let mut sample = template.clone();
    classify(&mut sample, None, defaults());
    assert_eq!(sample.impact, "SWAP THRASHING");

    let config: Config =
        serde_json::from_str(r#"{"swap_warn_rate":67108864,"swap_critical_rate":134217728}"#)
            .expect("valid json should parse");
    let mut sample = template;
    classify(&mut sample, None, Thresholds::from_config(&config));
    assert_ne!(sample.impact, "SWAP THRASHING");
    assert_eq!(sample.impact, "PAGING ACTIVE");
}

#[test]
fn configured_compression_rate_changes_the_classified_impact() {
    let template = Sample {
        rate_ready: true,
        total_memory: 32 * 1024 * 1024 * 1024,
        availability: Some(50),
        pressure: "GREEN".into(),
        vm_available: true,
        swap_available: true,
        thermal: "no warning".into(),
        llm_count: 1,
        compress: COMPRESSION_WARN_RATE,
        ..Sample::default()
    };

    let mut sample = template.clone();
    classify(&mut sample, None, defaults());
    assert_eq!(sample.impact, "COMPRESSION ACTIVE");

    let config: Config = serde_json::from_str(r#"{"compression_warn_rate":268435456}"#)
        .expect("valid json should parse");
    let mut sample = template;
    classify(&mut sample, None, Thresholds::from_config(&config));
    assert_eq!(sample.impact, "LLM READY");
}

#[test]
fn inverted_or_out_of_range_thresholds_are_clamped_into_usable_bands() {
    let config: Config = serde_json::from_str(
        r#"{"memory_warn_load":90,"memory_critical_load":40,
            "gpu_warn_load":250,"gpu_critical_load":10,
            "swap_warn_rate":1000,"swap_critical_rate":10,
            "compression_warn_rate":100,"compression_warn_exit":900}"#,
    )
    .expect("valid json should parse");
    let thresholds = Thresholds::from_config(&config);

    assert_eq!(thresholds.memory_warn_load, 90);
    assert_eq!(thresholds.memory_critical_load, 90);
    assert_eq!(thresholds.gpu_warn_load, 100);
    assert_eq!(thresholds.gpu_critical_load, 100);
    assert_eq!(thresholds.swap_warn_rate, 1000);
    assert_eq!(thresholds.swap_critical_rate, 1000);
    assert_eq!(thresholds.compression_warn_exit, 100);
    /* The warning band is never silently removed by bad input. */
    assert_eq!(ChartMetric::Memory.tone(89, thresholds), Tone::Green);
    assert_eq!(ChartMetric::Memory.tone(90, thresholds), Tone::Red);
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
fn partially_specified_endpoint_settings_keep_discovered_values() {
    let discovered = parse_omlx_server_env("OMLX_HOST=10.0.0.5\nOMLX_PORT=9000\n");

    let host_only: Config = serde_json::from_str(r#"{"omx":{"host":"192.168.1.20"}}"#)
        .expect("valid json should parse");
    assert_eq!(
        resolve_omlx_endpoint(&discovered, &host_only),
        ("192.168.1.20".to_owned(), 9000)
    );

    let port_only: Config =
        serde_json::from_str(r#"{"omx":{"port":8123}}"#).expect("valid json should parse");
    assert_eq!(
        resolve_omlx_endpoint(&discovered, &port_only),
        ("10.0.0.5".to_owned(), 8123)
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
fn out_of_range_config_values_fall_back_instead_of_refusing_to_start() {
    let valid: Config =
        serde_json::from_str(r#"{"interval":5,"history":500}"#).expect("valid json should parse");
    assert_eq!(config_interval(&valid), (5, false));
    assert_eq!(config_history(&valid), (500, false));

    let out_of_range: Config =
        serde_json::from_str(r#"{"interval":0,"history":5}"#).expect("valid json should parse");
    assert_eq!(config_interval(&out_of_range), (INTERVAL_DEFAULT, true));
    assert_eq!(config_history(&out_of_range), (HISTORY_DEFAULT, true));

    assert_eq!(
        config_interval(&Config::default()),
        (INTERVAL_DEFAULT, false)
    );
    assert_eq!(config_history(&Config::default()), (HISTORY_DEFAULT, false));
}

/**
 * Every field in `Config` must reach the value mlxtop actually runs with.
 *
 * The destructuring below is exhaustive on purpose: adding a field to
 * `Config` stops this test from compiling until the field is named here,
 * and an unused binding fails `clippy -D warnings`, so a new setting
 * cannot be merged without an assertion that something reads it.
 */
#[test]
fn every_config_field_reaches_the_resolved_settings() {
    let config: Config =
        serde_json::from_str(CONFIG_WITH_EVERY_FIELD_SET).expect("the fixture should parse");

    let Config {
        interval,
        history,
        omx,
        memory_warn_load,
        memory_critical_load,
        gpu_warn_load,
        gpu_critical_load,
        swap_warn_rate,
        swap_critical_rate,
        compression_warn_rate,
        swap_warn_exit,
        compression_warn_exit,
        gpu_warn_exit,
        alarms,
        journal,
    } = config.clone();
    let OmxConfig { host, port } = omx.expect("the fixture sets omx");

    assert_eq!(
        config_interval(&config).0,
        interval.expect("the fixture sets interval")
    );
    assert_eq!(
        config_history(&config).0,
        history.expect("the fixture sets history")
    );
    assert_eq!(
        resolve_omlx_endpoint(&DiscoveredEndpoint::default(), &config),
        (
            host.expect("the fixture sets omx.host"),
            port.expect("the fixture sets omx.port")
        )
    );

    let thresholds = Thresholds::from_config(&config);
    assert_eq!(
        thresholds,
        Thresholds {
            memory_warn_load: memory_warn_load.expect("fixture"),
            memory_critical_load: memory_critical_load.expect("fixture"),
            gpu_warn_load: gpu_warn_load.expect("fixture"),
            gpu_critical_load: gpu_critical_load.expect("fixture"),
            gpu_warn_exit: gpu_warn_exit.expect("fixture"),
            swap_warn_rate: swap_warn_rate.expect("fixture"),
            swap_critical_rate: swap_critical_rate.expect("fixture"),
            swap_warn_exit: swap_warn_exit.expect("fixture"),
            compression_warn_rate: compression_warn_rate.expect("fixture"),
            compression_warn_exit: compression_warn_exit.expect("fixture"),
        },
        "a Config field was parsed but never reached Thresholds"
    );

    let (commands, _command_receiver) = mpsc::channel();
    let (_view_sender, views) = mpsc::channel();
    let app = App::with_sampler(
        1,
        60,
        config.clone(),
        Sampler {
            commands,
            views,
            handle: None,
        },
    );
    assert_eq!(app.alarms, alarms.expect("the fixture sets alarms"));

    let journal = journal.expect("the fixture sets journal");
    let JournalConfig {
        prompts,
        llm,
        queue,
        memory,
        paging,
        gpu,
        thermal,
    } = journal.clone();
    for (toggle, state) in [
        (prompts, "PROMPT"),
        (llm, "LLM"),
        (queue, "QUEUE"),
        (memory, "PRESSURE"),
        (paging, "PAGING"),
        (gpu, "GPU"),
        (thermal, "THERMAL"),
    ] {
        assert_eq!(toggle, Some(false));
        assert!(
            !journal_records(&journal, state, false),
            "journal toggle for {state} was parsed but never filters"
        );
    }
}

/**
 * Every field in `Thresholds` must change something the user can see.
 *
 * This is the regression guard for the "deserialized and then ignored"
 * class of bug: each probe reports an observable result — a chart tone, a
 * load label, a classified impact — and the test fails unless configuring
 * the field changes it. Reverting any field to a hard-coded constant
 * fails here even though parsing still succeeds.
 *
 * The destructuring and the `all_fields` array are exhaustive on purpose:
 * a new `Thresholds` field stops the test compiling, and the length
 * assertion then fails until the field is given a probe below.
 */
#[test]
fn every_threshold_field_changes_an_observable_result() {
    struct Probe {
        field: &'static str,
        config: &'static str,
        observe: fn(Thresholds) -> String,
    }

    fn tone_name(tone: Tone) -> String {
        format!("{tone:?}")
    }

    fn impact_after_classify(
        thresholds: Thresholds,
        previous_impact: Option<&str>,
        prepare: fn(&mut Sample),
    ) -> String {
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
        prepare(&mut sample);
        let previous = previous_impact.map(|impact| Sample {
            impact: impact.into(),
            ..Sample::default()
        });
        classify(&mut sample, previous.as_ref(), thresholds);
        sample.impact
    }

    let probes = [
        Probe {
            field: "memory_warn_load",
            config: r#"{"memory_warn_load":55}"#,
            observe: |thresholds| tone_name(ChartMetric::Memory.tone(60, thresholds)),
        },
        Probe {
            field: "memory_critical_load",
            config: r#"{"memory_critical_load":95}"#,
            observe: |thresholds| tone_name(ChartMetric::Memory.tone(85, thresholds)),
        },
        Probe {
            field: "gpu_warn_load",
            config: r#"{"gpu_warn_load":70}"#,
            observe: |thresholds| gpu_load_label(Some(72), thresholds).to_owned(),
        },
        Probe {
            field: "gpu_critical_load",
            config: r#"{"gpu_warn_load":70,"gpu_critical_load":85}"#,
            observe: |thresholds| gpu_load_label(Some(88), thresholds).to_owned(),
        },
        Probe {
            field: "gpu_warn_exit",
            config: r#"{"gpu_warn_exit":60}"#,
            observe: |thresholds| {
                impact_after_classify(thresholds, Some("GPU BUSY"), |sample| {
                    sample.gpu_util = Some(65);
                })
            },
        },
        Probe {
            field: "swap_warn_rate",
            config: r#"{"swap_warn_rate":4194304}"#,
            observe: |thresholds| tone_name(ChartMetric::Swap.tone(2 * MIB, thresholds)),
        },
        Probe {
            field: "swap_critical_rate",
            config: r#"{"swap_critical_rate":33554432}"#,
            // Critical paging is a finding (both directions at the rate); the
            // paging chart's red follows that finding.
            observe: |thresholds| {
                impact_after_classify(thresholds, None, |sample| {
                    sample.swap_in = 16 * MIB;
                    sample.swap_out = 16 * MIB;
                })
            },
        },
        Probe {
            field: "swap_warn_exit",
            config: r#"{"swap_warn_exit":4194304}"#,
            observe: |thresholds| {
                impact_after_classify(thresholds, Some("PAGING ACTIVE"), |sample| {
                    sample.swap_in = 3 * MIB;
                })
            },
        },
        Probe {
            field: "compression_warn_rate",
            config: r#"{"compression_warn_rate":134217728}"#,
            observe: |thresholds| {
                impact_after_classify(thresholds, None, |sample| {
                    sample.compress = 64 * MIB;
                })
            },
        },
        Probe {
            field: "compression_warn_exit",
            config: r#"{"compression_warn_exit":50331648}"#,
            observe: |thresholds| {
                impact_after_classify(thresholds, Some("COMPRESSION ACTIVE"), |sample| {
                    sample.compress = 40 * MIB;
                })
            },
        },
    ];

    let Thresholds {
        memory_warn_load,
        memory_critical_load,
        gpu_warn_load,
        gpu_critical_load,
        gpu_warn_exit,
        swap_warn_rate,
        swap_critical_rate,
        swap_warn_exit,
        compression_warn_rate,
        compression_warn_exit,
    } = defaults();
    let all_fields = [
        memory_warn_load,
        memory_critical_load,
        gpu_warn_load,
        gpu_critical_load,
        gpu_warn_exit,
        swap_warn_rate,
        swap_critical_rate,
        swap_warn_exit,
        compression_warn_rate,
        compression_warn_exit,
    ];
    assert_eq!(
        probes.len(),
        all_fields.len(),
        "every field in Thresholds needs a probe proving it changes behaviour"
    );

    for probe in probes {
        let config: Config = serde_json::from_str(probe.config)
            .unwrap_or_else(|error| panic!("{} fixture should parse: {error}", probe.field));
        let configured = (probe.observe)(Thresholds::from_config(&config));
        let built_in = (probe.observe)(defaults());
        assert_ne!(
            configured, built_in,
            "setting {} changed nothing observable — it is parsed but not applied",
            probe.field
        );
    }
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
fn overview_time_series_share_window_and_zoom_while_request_bars_stay_independent() {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    for (width, height) in [(80, 24), (100, 40), (170, 42), (240, 60)] {
        for zoom in [1, 2, 4, 8] {
            app.charts.focused = Chart::Generation;
            app.charts.reset_zoom();
            while app.charts.zoom(Chart::Generation) < zoom {
                app.charts.change_zoom(true);
            }
            render_app(&app, width, height);
            let shared = app
                .charts
                .overview_samples
                .get()
                .unwrap()
                .div_ceil(usize::from(zoom));
            for (chart, area) in app.charts.regions.borrow().iter() {
                let gutter = match chart {
                    Chart::Generation | Chart::Prefill => 8,
                    Chart::Paging => 14,
                    Chart::Memory | Chart::Gpu | Chart::Cache => 7,
                    Chart::Queue => {
                        area.width
                            - operator_charts::queue_plot_width(
                                &app.collector.operator_history,
                                area.width,
                            )
                    }
                    _ => continue,
                };
                assert_eq!(
                    app.charts
                        .visible_samples(*chart, usize::from(area.width - gutter)),
                    shared,
                    "{chart:?} at {width}x{height}"
                );
                assert_eq!(app.charts.zoom(*chart), zoom);
            }
            assert_eq!(app.charts.zoom(Chart::Prompt), 1);
            assert_eq!(app.interval, Duration::from_secs(1));
        }
    }
}

#[test]
fn cache_never_replaces_missing_interval_samples_with_cumulative_gauge() {
    let mut app = test_app(0);
    app.collector.current.llm_cache_efficiency = Some(62.7);
    app.collector.current.llm_prefix_hit_rate = Some(40.5);
    for missing in [0, 4, 80] {
        let mut history = VecDeque::from([ChartPoint::new(Some(0), Tone::Cyan)]);
        history.extend(std::iter::repeat_n(
            ChartPoint::new(None, Tone::Muted),
            missing,
        ));
        let text = render_view(45, 10, |frame| {
            app.render_indicator_chart(frame, frame.area(), "cache", &history, ChartMetric::Cache)
        });
        assert!(text.contains("since start 62.7% · prefix 40.5%"), "{text}");
        assert!(
            !text.contains('█'),
            "aggregate must never become a gauge: {text}"
        );
        // The last measured interval stays marked LAST while it is in view;
        // past the window the reading is unknown and the panel says why.
        let reading = match missing {
            0 => "hit 0%",
            4 => "LAST 0%",
            _ => "hit —",
        };
        assert!(text.contains(reading), "{text}");
        if missing == 80 {
            assert!(text.contains("No prompts read in this window"), "{text}");
        }
    }
}

#[test]
fn server_rate_averages_are_secondary_details_in_expanded_charts() {
    let mut app = test_app(0);
    app.collector.current.llm_generation_tps = Some(35.7);
    app.collector.current.llm_source = TelemetrySource::Live;
    app.collector.current.llm_status = "idle".into();
    let history = VecDeque::from([
        ChartPoint::new(Some(247), Tone::Cyan),
        ChartPoint::new(None, Tone::Muted),
    ]);
    for expanded in [false, true] {
        app.charts.expanded = expanded;
        let text = render_view(90, 10, |frame| {
            app.render_indicator_chart(
                frame,
                frame.area(),
                "generation",
                &history,
                ChartMetric::Generation,
            )
        });
        assert!(text.contains("LAST 24.7 tok/s"), "{text}");
        assert_eq!(
            text.contains("SERVER AVG GEN 35.7 tok/s"),
            expanded,
            "{text}"
        );
    }
}

#[test]
fn overview_review_fixtures_keep_readouts_visible_at_supported_sizes() {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    for state in ["live", "idle", "missing"] {
        if state == "idle" {
            populate_single_idle_fixture(&mut app);
        }
        if state == "missing" {
            app = test_app(0);
        }
        for (width, height) in [(80, 24), (170, 42), (240, 60)] {
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| app.draw(frame)).unwrap();
            let buffer = terminal.backend().buffer();
            let screen = (0..height)
                .map(|y| {
                    (0..width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n");
            for label in [
                "memory",
                "generation",
                "prefill",
                "GPU",
                "cache",
                "queue",
                "? help q quit",
            ] {
                assert!(
                    screen.contains(label),
                    "missing {label} at {width}x{height}: {screen}"
                );
            }
            assert!(!screen.contains("process memory"));
            if let Ok(directory) = std::env::var("MLXTOP_REVIEW_DIR") {
                std::fs::create_dir_all(&directory).unwrap();
                let cells: Vec<_> = buffer.content.iter().map(|cell| serde_json::json!({
                    "text": cell.symbol(), "fg": cell.fg.to_string(), "bg": cell.bg.to_string(),
                    "bold": cell.modifier.contains(Modifier::BOLD),
                })).collect();
                let json = serde_json::json!({"width": width, "height": height, "cells": cells});
                std::fs::write(
                    format!("{directory}/{state}-{width}x{height}.json"),
                    json.to_string(),
                )
                .unwrap();
                std::fs::write(format!("{directory}/{state}-{width}x{height}.txt"), screen)
                    .unwrap();
            }
        }
    }
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

#[test]
fn remote_api_rates_are_not_correlated_with_local_gpu_or_memory() {
    let mut engine = CorrelationEngine::default();
    let sample = Sample {
        llm_remote: true,
        llm_generation_tps: Some(20.0),
        llm_generation_tps_live: true,
        gpu_util: Some(100),
        ..Sample::default()
    };
    let insight = engine.observe(&sample, defaults());
    assert_eq!(insight.direction, ThroughputDirection::Unknown);
    assert!(engine.speed.level().is_none());
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
        "20.5/36.0 GiB · 57%",
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
fn host_row_is_memory_cache_and_paging_and_compression_sits_above_the_queue() {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    let screen = render_app(&app, 120, 40);
    for label in ["PRESSURE normal", "wired 3.2 GiB"] {
        assert!(screen.contains(label), "missing {label}\n{screen}");
    }
    // The compressor's size is in the memory legend; a share of RAM says nothing new.
    assert!(!screen.contains("of RAM"), "{screen}");
    let regions = app.charts.regions.borrow().clone();
    let region = |chart| regions.iter().find(|(id, _)| *id == chart).map(|r| r.1);
    let memory = region(Chart::Memory).unwrap();
    let cache = region(Chart::Cache).unwrap();
    let paging = region(Chart::Paging).unwrap();
    assert_eq!((memory.y, cache.y), (paging.y, paging.y));
    assert!(memory.right() <= cache.x && cache.right() <= paging.x);
    let compression = region(Chart::Compression).unwrap();
    let queue = region(Chart::Queue).unwrap();
    assert_eq!(compression.y, region(Chart::Prompt).unwrap().y);
    assert_eq!(compression.x, queue.x);
    // Expanding compression keeps its own history and readings, the ratio among them.
    app.charts.focused = Chart::Compression;
    app.charts.expanded = true;
    let expanded = render_app(&app, 120, 40);
    assert!(expanded.contains("4.6 GiB stored in 1.6 GiB · 2.9× ratio"));
    assert!(
        expanded.contains("compressing 3.0 MiB/s · decompressing 1.5 MiB/s"),
        "{expanded}"
    );
    app.charts.expanded = false;

    // Linux exposes no compressor counters: the queue takes the column.
    app.collector.platform = Platform::Linux;
    render_app(&app, 120, 40);
    let regions = app.charts.regions.borrow().clone();
    assert!(regions
        .iter()
        .all(|(chart, _)| *chart != Chart::Compression));
    let find = |chart| regions.iter().find(|(c, _)| *c == chart).unwrap().1;
    assert_eq!(find(Chart::Memory).y, find(Chart::Cache).y);
    assert_eq!(find(Chart::Queue).y, find(Chart::Prompt).y);
}

#[test]
fn chart_colors_agree_with_the_assessment_at_every_threshold() {
    use crate::history::{compression_tone, paging_tone};
    let t = defaults();
    let host = Sample {
        rate_ready: true,
        vm_available: true,
        swap_available: true,
        total_memory: 32 * 1024 * MIB,
        availability: Some(50),
        pressure: "GREEN".into(),
        pressure_meaning: "normal".into(),
        gpu_util: Some(0),
        ..Sample::default()
    };
    let classified = |edit: &dyn Fn(&mut Sample), previous: Option<&Sample>| {
        let mut sample = host.clone();
        edit(&mut sample);
        crate::analysis::classify(&mut sample, previous, t);
        sample
    };
    // Paging: the chart's color never outranks the finding, and red appears
    // exactly with a critical paging finding.
    for (swap_in, swap_out, chart, impact) in [
        (0, 0, Tone::Green, "IDLE"),
        (512 * 1024, 0, Tone::Green, "IDLE"),
        (MIB, 0, Tone::Yellow, "WATCH PAGING"),
        (4 * MIB, 0, Tone::Yellow, "PAGING ACTIVE"),
        (15 * MIB, 0, Tone::Yellow, "PAGING ACTIVE"),
        (20 * MIB, 0, Tone::Red, "PAGING ACTIVE"),
        (16 * MIB, 16 * MIB, Tone::Red, "SWAP THRASHING"),
        (0, 32 * MIB, Tone::Red, "HEAVY PAGING"),
        (40 * MIB, 0, Tone::Red, "PAGE-IN RECOVERY"),
    ] {
        let sample = classified(
            &|s| {
                s.swap_in = swap_in;
                s.swap_out = swap_out;
            },
            None,
        );
        assert_eq!(sample.impact, impact, "{swap_in}/{swap_out}");
        let tone = paging_tone(swap_in + swap_out, &sample.impact, t);
        assert_eq!(tone, chart, "{swap_in}/{swap_out}");
        if impact != "IDLE" {
            assert_eq!(
                tone, sample.impact_tone,
                "chart and finding agree for {impact}"
            );
        }
    }
    // Compression: the chart keeps the finding's enter/exit hysteresis.
    let mut previous: Option<Sample> = None;
    let mut previous_tone = None;
    for (rate, active) in [
        (63 * MIB, false),
        (64 * MIB, true),
        (40 * MIB, true),
        (32 * MIB, true),
        (31 * MIB, false),
        (40 * MIB, false),
    ] {
        let sample = classified(&|s| s.compress = rate, previous.as_ref());
        let tone = compression_tone(rate, previous_tone, t);
        assert_eq!(sample.impact == "COMPRESSION ACTIVE", active, "{rate}");
        assert_eq!(
            tone == Tone::Yellow,
            active,
            "chart follows the finding at {rate}"
        );
        previous = Some(sample);
        previous_tone = Some(tone);
    }
    // GPU utilization uses its configured bands.
    for (load, tone) in [
        (74, Tone::Green),
        (75, Tone::Yellow),
        (89, Tone::Yellow),
        (90, Tone::Red),
    ] {
        assert_eq!(ChartMetric::Gpu.tone(load, t), tone);
    }
}

#[test]
fn throughput_and_cache_charts_grade_against_their_thresholds() {
    // Generation follows the assessment: a 30% drop is red whatever its cause.
    let mut insight = CorrelationInsight {
        direction: ThroughputDirection::Down,
        cause: CorrelationCause::Queueing,
        delta_percent: Some(-15.0),
        ..CorrelationInsight::default()
    };
    assert_eq!(insight.tone(), Tone::Yellow);
    insight.delta_percent = Some(-30.0);
    assert_eq!(insight.tone(), Tone::Red);

    // Cache reuse: green from 50%, yellow from 20%, red below.
    for (value, tone) in [
        (100, Tone::Green),
        (50, Tone::Green),
        (49, Tone::Yellow),
        (20, Tone::Yellow),
        (19, Tone::Red),
    ] {
        assert_eq!(ChartMetric::Cache.tone(value, defaults()), tone, "{value}");
    }
}

#[test]
fn levels_are_areas_and_bursty_rates_are_bars() {
    for metric in [ChartMetric::Memory, ChartMetric::Gpu] {
        assert_eq!(chart_form(metric), ChartForm::Area);
    }
    for metric in [
        ChartMetric::Compression,
        ChartMetric::Swap,
        ChartMetric::Generation,
        ChartMetric::Prefill,
        ChartMetric::Cache,
    ] {
        assert_eq!(chart_form(metric), ChartForm::Bars);
    }
}

fn render_chart(
    metric: ChartMetric,
    history: &VecDeque<ChartPoint>,
    width: u16,
) -> ratatui::buffer::Buffer {
    render_zoomed_chart(metric, history, width, 1)
}

fn render_zoomed_chart(
    metric: ChartMetric,
    history: &VecDeque<ChartPoint>,
    width: u16,
    zoom: u16,
) -> ratatui::buffer::Buffer {
    let mut app = test_app(0);
    app.charts.focused = Chart::from(metric);
    while app.charts.zoom(Chart::from(metric)) < zoom {
        app.charts.change_zoom(true);
    }
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(width, 12)).unwrap();
    terminal
        .draw(|frame| app.render_indicator_chart(frame, frame.area(), "chart", history, metric))
        .unwrap();
    terminal.backend().buffer().clone()
}

#[test]
fn bars_keep_a_gap_between_samples_when_there_is_room_and_show_raw_heights() {
    // At 1× each sample has one column; zoomed in, a column between samples
    // stays empty so each bar reads on its own. Solid cells are green backgrounds.
    let history = VecDeque::from(vec![ChartPoint::new(Some(400), Tone::Green); 8]);
    let buffer = render_zoomed_chart(ChartMetric::Generation, &history, 90, 4);
    let row = (0..12)
        .map(|y| {
            (0..90)
                .map(|x| buffer[(x, y)].bg == GREEN)
                .collect::<Vec<_>>()
        })
        .find(|row| row.iter().any(|filled| *filled))
        .expect("bars drawn");
    let runs = row
        .split(|filled| !filled)
        .filter(|run| !run.is_empty())
        .count();
    assert_eq!(runs, 8, "one solid run per sample: {row:?}");
    let unzoomed = render_chart(ChartMetric::Generation, &history, 90);
    let solid = (0..90).filter(|x| (0..12).any(|y| unzoomed[(*x, y)].bg == GREEN));
    assert_eq!(solid.count(), 8, "one column per sample, adjacent");

    // Two close rates a line would hold level at the same height stay distinct.
    let history = VecDeque::from(vec![
        ChartPoint::new(Some(400), Tone::Green),
        ChartPoint::new(Some(430), Tone::Green),
    ]);
    let buffer = render_chart(ChartMetric::Generation, &history, 40);
    // Height in eighths: full (filled) cells plus the block that tops them.
    let eighths: Vec<usize> = (0..40)
        .map(|x| {
            (0..12)
                .map(|y| {
                    let cell = &buffer[(x, y)];
                    if cell.bg == GREEN {
                        8
                    } else {
                        "▁▂▃▄▅▆▇"
                            .chars()
                            .position(|glyph| cell.symbol().starts_with(glyph))
                            .map_or(0, |index| index + 1)
                    }
                })
                .sum()
        })
        .filter(|height| *height > 0)
        .collect();
    assert_eq!(eighths.len(), 2);
    assert!(eighths[1] > eighths[0], "{eighths:?}");
}

#[test]
fn areas_fill_beneath_their_graded_line_in_one_quiet_colour() {
    let body = crate::theme::AREA_FILL;
    // Busy samples flipping between the warning and critical grades: the line
    // keeps each grade, the body stays one colour instead of striping.
    let history: VecDeque<_> = (0..40)
        .map(|i| {
            if i % 2 == 0 {
                ChartPoint::new(Some(100), Tone::Red)
            } else {
                ChartPoint::new(Some(88), Tone::Yellow)
            }
        })
        .collect();
    let buffer = render_chart(ChartMetric::Gpu, &history, 50);
    let bottom = (0..12)
        .rev()
        .find(|y| buffer[(47, *y)].bg == body)
        .expect("area body drawn");
    assert!(
        (20..48).all(|x| buffer[(x, bottom)].bg == body),
        "one body colour"
    );

    let history = VecDeque::from(vec![ChartPoint::new(Some(60), Tone::Yellow); 40]);
    let buffer = render_chart(ChartMetric::Gpu, &history, 50);
    let column = 47;
    let line = (0..12)
        .find(|y| buffer[(column, *y)].symbol() == "━")
        .expect("line drawn");
    assert_eq!(
        buffer[(column, line)].fg,
        YELLOW,
        "the line carries the grade"
    );
    assert_eq!(
        buffer[(column, line)].bg,
        body,
        "the line is the area's top edge"
    );
    assert!((line + 1..11).all(|y| buffer[(column, y)].bg == body));
    assert!(
        (0..line).all(|y| buffer[(column, y)].bg != body),
        "nothing above it"
    );
}

#[test]
fn meters_fill_in_eighths_and_the_wide_process_table_shows_one_per_share() {
    assert_eq!(meter_cells(0.0, 3), "   ");
    assert_eq!(meter_cells(100.0, 3), "███");
    assert_eq!(meter_cells(50.0, 3), "█▌ ");
    assert_eq!(meter_cells(4.0, 3), "▏  ", "a small share still shows");
    assert_eq!(meter_cells(250.0, 2), "██");

    let mut app = test_app(1);
    app.collector.current.llm_processes = vec![LlmProcess {
        pid: 42,
        name: "omlx-server".into(),
        command: "/usr/local/bin/omlx-server --port 8080".into(),
        rss: 14 << 30,
        cpu: 3.0,
        memory_percent: Some(40.0),
        state: "S".into(),
        pageins: None,
        pagein_rate: None,
    }];
    let rendered = render_app(&app, 180, 40);
    assert!(rendered.contains("MEM%"), "{rendered}");
    // 40% of three cells is 9.6 eighths: one full cell and a quarter.
    assert!(rendered.contains("█▎  40.0%"), "{rendered}");
}

#[test]
fn paging_scales_to_its_warning_rate_so_a_trickle_stays_small() {
    // 124 KiB/s of steady page-ins: well under the 1 MiB/s warning rate.
    let history = VecDeque::from(vec![ChartPoint::new(Some(124 * 1024), Tone::Green); 40]);
    let buffer = render_chart(ChartMetric::Swap, &history, 60);
    let column = 57;
    let bar = (0..12).filter(|y| buffer[(column, *y)].bg == GREEN).count();
    assert!(bar <= 2, "a trickle is a short bar, not a slab: {bar} rows");
    // The warning rate is the reference line, yellow and labelled with its value.
    let guide = (0..12)
        .find(|y| buffer[(40, *y)].symbol() == "┄")
        .expect("warning line drawn");
    assert_eq!(buffer[(40, guide)].fg, YELLOW);
    let label: String = (0..14).map(|x| buffer[(x, guide)].symbol()).collect();
    assert!(label.contains("1.0 MiB/s"), "{label}");
}

#[test]
fn short_rate_panels_draw_a_sparkline_instead_of_a_hint() {
    let history: VecDeque<_> = (0..30)
        .map(|i| ChartPoint::new((i % 3 != 0).then_some(300 + i as u64 * 5), Tone::Green))
        .collect();
    let app = test_app(0);
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(40, 4)).unwrap();
    terminal
        .draw(|frame| {
            app.render_indicator_chart(
                frame,
                frame.area(),
                "generation",
                &history,
                ChartMetric::Generation,
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(!text.contains("Enter: history"), "{text}");
    // Bars in the sample's grade, with gaps where nothing was generating.
    let columns: Vec<bool> = (1..39)
        .map(|x| (1..3).any(|y| buffer[(x, y)].bg == GREEN))
        .collect();
    assert!(
        columns.iter().filter(|filled| **filled).count() >= 15,
        "{columns:?}"
    );
    assert!(
        columns.windows(2).any(|pair| pair[0] != pair[1]),
        "idle samples stay empty"
    );
}

#[test]
fn memory_history_stacks_what_ram_holds_over_a_pressure_ribbon() {
    let gib = 1024 * MIB;
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    // A model loads halfway: wired jumps, cache is squeezed; pressure turns yellow.
    app.collector.load_history = (0..40)
        .map(|i| ChartPoint::new(Some(90), if i < 20 { Tone::Green } else { Tone::Yellow }))
        .collect();
    app.collector.composition_history = (0..40)
        .map(|i| {
            Some(if i < 20 {
                [4 * gib, 12 * gib, 2 * gib, 14 * gib, 4 * gib]
            } else {
                [20 * gib, 6 * gib, 2 * gib, 4 * gib, 4 * gib]
            })
        })
        .collect();
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(60, 20)).unwrap();
    terminal
        .draw(|frame| {
            app.render_indicator_chart(
                frame,
                frame.area(),
                "memory",
                &app.collector.load_history,
                ChartMetric::Memory,
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let fills = crate::memory_composition::history_colors();
    let wired_rows = |x: u16| (0..20).filter(|y| buffer[(x, *y)].bg == fills[0]).count();
    // Before the load little is wired; after it, much more.
    assert!(
        wired_rows(57) > wired_rows(30) + 2,
        "{} vs {}",
        wired_rows(57),
        wired_rows(30)
    );
    // The ribbon under the stack carries each sample's pressure grade.
    let ribbon = (0..20)
        .rev()
        .find(|y| buffer[(57, *y)].symbol() == "▀")
        .expect("pressure ribbon");
    assert_eq!(buffer[(57, ribbon)].fg, YELLOW);
    assert_eq!(buffer[(30, ribbon)].fg, GREEN);
    // No line: the stack's top is the level.
    assert!((0..20).all(|y| buffer[(57, y)].symbol() != "━"));

    // Without composition counters (Linux), memory stays an area chart.
    app.collector.composition_history = std::iter::repeat_n(None, 40).collect();
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(60, 20)).unwrap();
    terminal
        .draw(|frame| {
            app.render_indicator_chart(
                frame,
                frame.area(),
                "memory",
                &app.collector.load_history,
                ChartMetric::Memory,
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert!((0..20).any(|y| buffer[(57, y)].symbol() == "━"));
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
fn rate_headlines_match_their_charts_and_name_a_summed_rate() {
    // Two requests decode while the prefill that came before them is over:
    // the screenshot that read "AVG PREFILL 209.0" above a 71.7 bar.
    let mut app = test_app(0);
    let request = |id: &str| domain::RequestUsage {
        provider: "oMLX".into(),
        model: "m".into(),
        id: id.into(),
        prompt: 50_705,
        cached: None,
        output: Some(15),
        output_tps: Some(21.8),
        completed: false,
        queued: false,
        prefilled: None,
        idle_for: None,
        ttft_ms: None,
        observed_at: Some(SystemTime::now()),
    };
    app.collector.current = Sample {
        llm_source: TelemetrySource::Live,
        llm_status: "generating".into(),
        llm_generation_tps: Some(43.6),
        llm_generation_tps_live: true,
        llm_prefill_tps: Some(209.0),
        llm_requests: vec![request("a"), request("b")],
        ..Sample::default()
    };
    let generation = VecDeque::from([ChartPoint::new(Some(436), Tone::Green)]);
    let text = render_view(60, 8, |frame| {
        app.render_indicator_chart(
            frame,
            frame.area(),
            "generation",
            &generation,
            ChartMetric::Generation,
        )
    });
    assert!(text.contains("LIVE 43.6 tok/s"), "{text}");
    assert!(text.contains("tok/s · total of 2 requests"), "{text}");

    let mut last = ChartPoint::new(Some(717), Tone::Green);
    last.observed_at = SystemTime::now() - Duration::from_secs(1);
    let prefill = VecDeque::from([last, ChartPoint::new(None, Tone::Muted)]);
    let text = render_view(60, 8, |frame| {
        app.render_indicator_chart(
            frame,
            frame.area(),
            "prefill",
            &prefill,
            ChartMetric::Prefill,
        )
    });
    // The headline is the plotted sample. The server's lifetime average is
    // a different figure; it lives in diagnostics, not beside the chart.
    assert!(text.contains("LAST 71.7 tok/s · 1s old"), "{text}");
    assert!(!text.contains("server avg"), "{text}");
    assert!(!text.contains("AVG PREFILL"), "{text}");
}

/// One poll of a lone oMLX request, as the decode tracker leaves it: its
/// speed since the last poll and how long since it produced a token.
fn decoding(second: u64, tps: f64, out: u64, idle: f64, gpu: u8) -> Sample {
    Sample {
        llm_provider: "oMLX".into(),
        llm_model: "Qwen3.8-27B-oQ4e-mtp".into(),
        llm_source: TelemetrySource::Live,
        llm_status: "generating".into(),
        llm_generation_tps: Some(tps),
        llm_generation_tps_live: true,
        llm_active_requests: Some(1),
        llm_observed_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(second)),
        gpu_util: Some(gpu),
        gpu_in_use: Some(97),
        gpu_alloc: Some(100),
        llm_requests: vec![domain::RequestUsage {
            provider: "oMLX".into(),
            model: "Qwen3.8-27B-oQ4e-mtp".into(),
            id: "e28cde".into(),
            prompt: 67_793,
            cached: Some(67_776),
            output: Some(out),
            output_tps: Some(tps),
            completed: false,
            queued: false,
            prefilled: None,
            idle_for: Some(Duration::from_secs_f64(idle)),
            ttft_ms: None,
            observed_at: None,
        }],
        ..Sample::default()
    }
}

/// Per-second speeds of one captured oMLX request (speculative decoding):
/// normal for this model, and never a slowdown on their own.
const CAPTURED_SPEEDS: [f64; 17] = [
    65.1, 37.5, 38.4, 46.3, 52.2, 40.7, 49.5, 49.3, 37.5, 40.8, 32.5, 44.6, 58.4, 29.6, 42.4, 44.4,
    27.6,
];

#[test]
fn speculative_decoding_jitter_within_a_request_is_not_a_slowdown() {
    let mut engine = CorrelationEngine::default();
    for (second, tps) in CAPTURED_SPEEDS.iter().enumerate() {
        let insight = engine.observe(
            &decoding(second as u64, *tps, 1_000 + second as u64 * 45, 0.05, 99),
            defaults(),
        );
        assert!(
            !insight.is_material_drop(),
            "second {second} at {tps} tok/s: {}",
            insight.summary
        );
    }
}

#[test]
fn a_stall_is_named_with_what_the_host_was_doing_and_journaled_with_its_end() {
    // The captured stall: 1,693 tokens out, then none for four polls while
    // the GPU sat idle and Metal memory stayed at 97%, then 31 tok/s again.
    let mut engine = CorrelationEngine::default();
    let mut samples = Vec::new();
    for (second, tps) in CAPTURED_SPEEDS.iter().enumerate() {
        samples.push(decoding(
            second as u64,
            *tps,
            1_000 + second as u64 * 40,
            0.05,
            99,
        ));
    }
    for (offset, idle) in [1.38, 2.39, 3.41, 4.41].iter().enumerate() {
        samples.push(decoding(17 + offset as u64, 0.0, 1_693, *idle, 0));
    }
    samples.push(decoding(21, 31.0, 1_724, 0.01, 99));
    for sample in &mut samples {
        sample.correlation = engine.observe(sample, defaults());
    }
    // Before 1.5 s without a token there is no verdict.
    assert!(!samples[17].correlation.is_material_drop());
    let stall = &samples[18].correlation;
    assert_eq!(stall.cause, CorrelationCause::Stall);
    assert_eq!(stall.lead, "no new token for 2s at 1.7k out · GPU idle");
    // A busy GPU and a full Metal heap hold all along; neither is the reason.
    assert!(!stall.summary.contains("Metal"), "{}", stall.summary);
    assert_eq!(stall.tone(), Tone::Red);
    assert!(samples[21].correlation.stall_seconds.is_none());

    // The Journal: one STALL row with the next step, one RESUMED row.
    let mut collector = Collector::with_host(
        60,
        offline_config(),
        Box::new(FakeHost::default()),
        Platform::MacOs,
        None,
    );
    for pair in samples.windows(2) {
        collector.record_journal_events(Some(&pair[0]), &pair[1]);
    }
    let rows: Vec<(String, String)> = collector
        .signals
        .iter()
        .map(|event| (event.state.clone(), event.summary.clone()))
        .filter(|(state, _)| matches!(state.as_str(), "STALL" | "RESUMED" | "SLOWDOWN"))
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                "STALL".to_owned(),
                "no new token for 2s at 1.7k out · GPU idle → Runtime idle mid-answer; check its log at this time.".to_owned()
            ),
            (
                "RESUMED".to_owned(),
                "generation resumed at 31.0 tok/s after 4s".to_owned()
            ),
        ]
    );

    // The chart says why under the bars that show it, in the anomaly colour.
    let mut app = test_app(0);
    app.collector.current = samples[20].clone();
    let history = VecDeque::from([
        ChartPoint::new(Some(420), Tone::Green),
        ChartPoint::new(Some(0), Tone::Red),
    ]);
    let buffer = {
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(60, 8)).unwrap();
        terminal
            .draw(|frame| {
                app.render_indicator_chart(
                    frame,
                    frame.area(),
                    "generation",
                    &history,
                    ChartMetric::Generation,
                )
            })
            .unwrap();
        terminal.backend().buffer().clone()
    };
    let bottom: String = (0..60).map(|x| buffer[(x, 7)].symbol()).collect();
    assert!(
        bottom.contains("▼ no new token for 4s at 1.7k out · GPU idle"),
        "{bottom}"
    );
    let x = bottom
        .find('▼')
        .map(|byte| bottom[..byte].chars().count())
        .unwrap() as u16;
    assert_eq!(buffer[(x, 7)].fg, UNUSUAL);
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

// --- runtime disk and SSD cache ------------------------------------------------

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

#[test]
fn disk_activity_is_storage_rates_and_cache_blocks_since_the_last_sample() {
    let at = Instant::now();
    let reading = |read: u64, written: u64, seconds: u64, started: u64| process_memory::Reading {
        pid: 7,
        started,
        resident: 1,
        footprint: 1,
        peak: 1,
        disk_read: read,
        disk_written: written,
        at: at + Duration::from_secs(seconds),
    };
    let cache = |loads, saves, evictions| domain::SsdCache {
        used: 1,
        capacity: 2,
        loads,
        saves,
        evictions,
        ..Default::default()
    };
    let first = reading(0, 0, 0, 1);
    let next = reading(800 * MIB, 4 * MIB, 2, 1);
    let activity = crate::collector::disk_activity(
        Some(&next),
        Some(&first),
        Some(cache(12, 3, 1)),
        Some(cache(2, 3, 0)),
    );
    assert_eq!(
        activity,
        domain::DiskActivity {
            read: Some(400 * MIB),
            write: Some(2 * MIB),
            loads: 10,
            saves: 0,
            evictions: 1,
            weights: 0,
        }
    );
    assert_eq!(activity.rate(), Some(402 * MIB));
    // A restarted runtime (new start time, counters reset) reads as nothing.
    let restarted = crate::collector::disk_activity(
        Some(&reading(0, 0, 3, 2)),
        Some(&next),
        Some(cache(0, 0, 0)),
        Some(cache(12, 3, 1)),
    );
    assert_eq!(restarted, domain::DiskActivity::default());
    assert_eq!(restarted.rate(), None);
    assert_eq!(process_memory::disk_rates(&first, Some(&first)), None);
    assert_eq!(process_memory::disk_rates(&first, None), None);
}

fn with_disk(mut sample: Sample, read: u64, loads: u64) -> Sample {
    sample.disk = domain::DiskActivity {
        read: Some(read),
        write: Some(0),
        loads,
        ..Default::default()
    };
    sample
}

#[test]
fn a_stall_beside_cache_reads_names_the_disk_and_points_at_it() {
    let mut engine = CorrelationEngine::default();
    let mut samples: Vec<Sample> = CAPTURED_SPEEDS
        .iter()
        .enumerate()
        .map(|(second, tps)| decoding(second as u64, *tps, 1_000 + second as u64 * 40, 0.05, 99))
        .collect();
    samples.push(with_disk(decoding(17, 0.0, 1_693, 1.2, 0), 900 * MIB, 40));
    samples.push(with_disk(decoding(18, 0.0, 1_693, 2.3, 0), 850 * MIB, 38));
    samples.last_mut().unwrap().ssd_cache = Some(domain::SsdCache {
        used: 100,
        capacity: 100,
        ..Default::default()
    });
    samples.last_mut().unwrap().disk.evictions = 5;
    for sample in &mut samples {
        sample.correlation = engine.observe(sample, defaults());
    }
    let stall = &samples[18].correlation;
    assert_eq!(stall.cause, CorrelationCause::Stall);
    assert_eq!(
        stall.lead,
        "no new token for 2s at 1.7k out · GPU idle · disk read 850.0 MiB/s · 38 SSD cache blocks loaded · SSD cache full"
    );
    assert_eq!(
        crate::diagnosis::slowdown_next_step(&samples[18]),
        "SSD cache is full and evicting; enlarge it or free the disk."
    );
    let mut not_full = samples[18].clone();
    not_full.correlation.lead = "no new token for 2s · disk read 850.0 MiB/s".into();
    assert_eq!(
        crate::diagnosis::slowdown_next_step(&not_full),
        "Runtime waited on its disk cache; check that disk's speed."
    );
    not_full.correlation.cause = CorrelationCause::DiskIo;
    not_full.correlation.lead = "1 SSD cache block saved".into();
    assert_eq!(
        crate::diagnosis::slowdown_next_step(&not_full),
        "Runtime waited on its disk cache; check that disk's speed."
    );
    not_full.correlation.lead = "x".into();
    assert_eq!(
        crate::diagnosis::slowdown_next_step(&not_full),
        "Runtime used its disk mid-answer; check its cache disk."
    );
}

#[test]
fn the_cache_panel_shows_what_the_cache_saved_over_its_disk_traffic() {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    // Without disk readings the panel keeps each interval's reuse.
    assert_eq!(app.cache_panel().0, "cache");
    app.collector.current.ssd_cache = Some(domain::SsdCache {
        used: 120 * 1024 * MIB,
        capacity: 120 * 1024 * MIB,
        ..Default::default()
    });
    app.collector.current.disk = domain::DiskActivity {
        read: Some(850 * MIB),
        write: Some(2 * MIB),
        loads: 38,
        ..Default::default()
    };
    // 1.2M prompt tokens this session, 1.08M served from the cache, measured
    // prefill at 200 tok/s: the cache saved 1h 30m of prefill.
    app.collector.current.llm_total_prompt_tokens = Some(1_200_000);
    app.collector.current.llm_total_cached_tokens = Some(1_080_000);
    app.collector.prefill_history.clear();
    for _ in 0..5 {
        push_history_with_tone(
            &mut app.collector.prefill_history,
            Some(2000),
            Tone::Green,
            60,
        );
    }
    app.collector.current.llm_requests = vec![RequestUsage {
        prompt: 29_000,
        cached: Some(26_000),
        ..RequestUsage::default()
    }];
    for second in 0..30 {
        push_history_with_tone(
            &mut app.collector.disk_history,
            Some(if second % 10 == 0 { 850 * MIB } else { 0 }),
            Tone::Cyan,
            60,
        );
    }
    assert!(app.cache_panel().2 == ChartMetric::Disk);
    let (name, history, metric) = app.cache_panel();
    let disk_history = history.clone();
    let history = &disk_history;
    let tall = render_view(70, 10, |frame| {
        app.render_indicator_chart(frame, frame.area(), name, history, metric)
    });
    for label in [
        "disk I/O",
        "KV hit 90% · ≈1h 30m saved",
        "session",
        "90% of 1.2M · ≈1h 30m saved",
        "last",
        "90% of 29.0k · ≈2m 10s saved",
        "loading prefixes from SSD",
        "SSD 120.0 GiB · 0 blocks loaded · 0 saved · 0 evicted",
    ] {
        assert!(tall.contains(label), "missing {label}\n{tall}");
    }
    // A short panel keeps the SSD row and draws the traffic, never a sentence.
    let short = render_view(40, 5, |frame| {
        app.render_indicator_chart(frame, frame.area(), name, history, metric)
    });
    assert!(short.contains("KV hit 90%"), "{short}");
    assert!(short.contains("SSD"), "{short}");
    // The Overview and the enlarged view both use it.
    let screen = render_app(&app, 180, 50);
    assert!(screen.contains("disk I/O"), "{screen}");
    app.charts.focused = Chart::Cache;
    app.charts.expanded = true;
    assert!(render_app(&app, 120, 40).contains("disk I/O"));
}

#[test]
fn cache_impact_reads_activity_speed_and_shares() {
    use crate::cache_impact::{activity, headline, last_prompt, prefill_speed, session};
    let mut sample = Sample::default();
    let none = VecDeque::new();
    assert_eq!(prefill_speed(&none, &sample), None);
    sample.llm_prefill_tps = Some(150.0);
    assert_eq!(prefill_speed(&none, &sample), Some(150.0));
    assert_eq!(headline(&sample, None, 60), "—");
    sample.llm_cache_efficiency = Some(62.0);
    assert_eq!(headline(&sample, None, 60), "KV hit 62%");
    sample.llm_total_prompt_tokens = Some(1000);
    sample.llm_total_cached_tokens = Some(600);
    assert_eq!(session(&sample), Some((1000, 600)));
    assert_eq!(headline(&sample, None, 60), "KV hit 60%");
    assert_eq!(headline(&sample, Some(10.0), 30), "KV hit 60%");
    assert_eq!(
        headline(&sample, Some(10.0), 60),
        "KV hit 60% · ≈1m 0s saved"
    );
    // The SSD tier's own counts split the hits into memory and disk.
    sample.ssd_cache = Some(domain::SsdCache {
        capacity: 1,
        hit_tokens: Some(600),
        disk_hit_tokens: Some(1),
        ..Default::default()
    });
    assert_eq!(
        crate::cache_impact::tiers(&sample),
        Some(crate::cache_impact::Tiers {
            prompt: 1000,
            memory: 599,
            disk: 1
        })
    );
    let bar = render_view(60, 1, |frame| {
        crate::cache_impact::draw_tiers(
            frame,
            frame.area(),
            crate::cache_impact::tiers(&sample).unwrap(),
            Some(10.0),
        )
    });
    assert!(bar.contains("RAM 60% · SSD <1% · ≈1m 0s saved"), "{bar}");
    sample.ssd_cache = None;
    sample.llm_total_cached_tokens = Some(2000);
    assert_eq!(
        session(&sample),
        None,
        "more cached than sent is not believed"
    );
    sample.llm_requests = vec![
        RequestUsage {
            prompt: 100,
            cached: Some(40),
            ..RequestUsage::default()
        },
        RequestUsage {
            prompt: 0,
            cached: Some(0),
            ..RequestUsage::default()
        },
    ];
    assert_eq!(last_prompt(&sample), Some((100, 40)));
    assert_eq!(activity(&sample), None);
    for (disk, state) in [
        (
            domain::DiskActivity {
                evictions: 1,
                ..Default::default()
            },
            "evicting old prefixes",
        ),
        (
            domain::DiskActivity {
                loads: 1,
                saves: 1,
                ..Default::default()
            },
            "loading and saving prefixes",
        ),
        (
            domain::DiskActivity {
                saves: 1,
                ..Default::default()
            },
            "saving prefixes to SSD",
        ),
        (
            domain::DiskActivity {
                read: Some(0),
                write: Some(0),
                ..Default::default()
            },
            "disk idle",
        ),
        (
            domain::DiskActivity {
                read: Some(400 * MIB),
                write: Some(0),
                ..Default::default()
            },
            "disk busy",
        ),
        (
            domain::DiskActivity {
                read: Some(MIB),
                write: Some(0),
                ..Default::default()
            },
            "light disk traffic",
        ),
    ] {
        sample.disk = disk;
        assert_eq!(activity(&sample).map(|(s, _)| s), Some(state));
    }
}

#[test]
fn the_ssd_row_shows_fill_or_says_the_cache_is_off() {
    let mut sample = Sample::default();
    let row = |sample: &Sample| {
        render_view(40, 1, |frame| {
            crate::swap_usage::draw_ssd_cache(frame, frame.area(), sample)
        })
    };
    assert!(row(&sample).contains("SSD cache off"));
    sample.ssd_cache = Some(domain::SsdCache {
        used: 30 * 1024 * MIB,
        capacity: 120 * 1024 * MIB,
        ..Default::default()
    });
    // An external KV cache is meant to fill: its size and what it did, no fill bar.
    sample.ssd_cache = Some(domain::SsdCache {
        used: 30 * 1024 * MIB,
        capacity: 120 * 1024 * MIB,
        loads: 1520,
        saves: 1314,
        evictions: 327,
        ..Default::default()
    });
    assert!(
        row(&sample).contains("SSD 120.0 GiB · ↑1.5k ↓1.3k ✕327"),
        "{}",
        row(&sample)
    );
    let wide = render_view(80, 1, |frame| {
        crate::swap_usage::draw_ssd_cache(frame, frame.area(), &sample)
    });
    assert!(
        wide.contains("1.5k blocks loaded · 1.3k saved · 327 evicted"),
        "{wide}"
    );
    render_view(0, 0, |frame| {
        crate::swap_usage::draw_ssd_cache(frame, Rect::new(0, 0, 0, 0), &sample)
    });
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

/// A lone request decoding steadily, with swap traffic at `swap` bytes/s.
fn paging(second: u64, tps: f64, swap: u64) -> Sample {
    let mut sample = decoding(second, tps, 500 + second * 40, 0.05, 99);
    sample.swap_in = swap;
    sample.rate_ready = true;
    sample
}

fn observe_all(engine: &mut CorrelationEngine, samples: &mut [Sample]) {
    for sample in samples.iter_mut() {
        sample.correlation = engine.observe(sample, defaults());
    }
}

fn journal_rows(samples: &[Sample], states: &[&str]) -> Vec<(String, String)> {
    let mut collector = Collector::with_host(
        200,
        offline_config(),
        Box::new(FakeHost::default()),
        Platform::MacOs,
        None,
    );
    for pair in samples.windows(2) {
        collector.record_journal_events(Some(&pair[0]), &pair[1]);
    }
    collector
        .signals
        .iter()
        .map(|event| (event.state.clone(), event.summary.clone()))
        .filter(|(state, _)| states.contains(&state.as_str()))
        .collect()
}

/// A sample halfway through a slowdown attributed to swap traffic.
fn slowed_by_paging() -> Sample {
    let mut sample = paging(30, 25.0, defaults().swap_warn_rate * 2);
    sample.vm_available = true;
    sample.correlation = CorrelationInsight {
        direction: ThroughputDirection::Down,
        cause: CorrelationCause::Paging,
        summary: "GEN ↓50.0% (50.0→25.0 tok/s) · unusual: paging 2.0 MiB/s (usual none)".into(),
        lead: "paging 2.0 MiB/s (usual none)".into(),
        delta_percent: Some(-50.0),
        slow: true,
        unusual: vec![CorrelationCause::Paging],
        episode: Some(DropEpisode {
            reference: 50.0,
            low: 25.0,
            seconds: 12.0,
            samples: 6,
        }),
        ..CorrelationInsight::default()
    };
    sample
}

/// Ten samples at `normal`, then six at `slow` taken during a slowdown.
fn history_with_drop(normal: u64, slow: u64, tone: Tone) -> VecDeque<ChartPoint> {
    let mut history: VecDeque<ChartPoint> = (0..10)
        .map(|_| ChartPoint::new(Some(normal), Tone::Green))
        .collect();
    history.extend((0..6).map(|_| ChartPoint::new(Some(slow), tone)));
    history
}

fn chart_buffer(
    app: &App,
    width: u16,
    height: u16,
    name: &'static str,
    history: &VecDeque<ChartPoint>,
    metric: ChartMetric,
) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| app.render_indicator_chart(frame, frame.area(), name, history, metric))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn buffer_rows(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn a_slowdown_draws_its_usual_speed_and_its_samples_in_the_anomaly_colour() {
    let mut app = test_app(0);
    app.collector.current = slowed_by_paging();
    let generation = history_with_drop(500, 250, Tone::Unusual);
    let buffer = chart_buffer(
        &app,
        60,
        10,
        "generation",
        &generation,
        ChartMetric::Generation,
    );
    let rows = buffer_rows(&buffer);
    let text = rows.join("\n");
    // What else is unusual sits under the bars, in the anomaly colour.
    let bottom = &rows[9];
    assert!(bottom.contains("▼ paging 2.0 MiB/s (usual none)"), "{text}");
    let x = bottom
        .find('▼')
        .map(|byte| bottom[..byte].chars().count())
        .unwrap() as u16;
    assert_eq!(buffer[(x, 9)].fg, UNUSUAL);
    // The usual speed is a dashed line with its value on the axis, visible
    // exactly where the bars fall short of it.
    let usual = rows
        .iter()
        .position(|row| row.contains('┄'))
        .expect("a line at the usual speed");
    assert!(
        rows[usual]
            .trim_start_matches('│')
            .trim_start()
            .starts_with("50 "),
        "{text}"
    );
    let dashes: Vec<u16> = (0..60)
        .filter(|x| buffer[(*x, usual as u16)].symbol() == "┄")
        .collect();
    assert!(dashes
        .iter()
        .all(|x| buffer[(*x, usual as u16)].fg == UNUSUAL));
    assert!((53..59).all(|x| dashes.contains(&x)), "{dashes:?}");
    assert!((43..53).all(|x| !dashes.contains(&x)), "{dashes:?}");
    // The slow samples are the only ones in the anomaly colour, and nothing
    // is tinted behind the bars.
    assert!((53..59).all(|x| buffer[(x, 8)].fg == UNUSUAL));
    assert!((43..53).all(|x| buffer[(x, 8)].fg == GREEN));
    assert!((43..59).all(|x| buffer[(x, 1)].bg == PANEL));

    // A chart whose signal was usual carries no mark at all.
    let gpu = history_with_drop(99, 40, Tone::Green);
    let buffer = chart_buffer(&app, 60, 10, "GPU", &gpu, ChartMetric::Gpu);
    assert!(!buffer_rows(&buffer).join("\n").contains('▼'));
    assert!((0..60).all(|x| (0..10).all(|y| buffer[(x, y)].fg != UNUSUAL)));

    // With nothing else unusual the chart says so instead of leaving the
    // arrow alone, also on a narrow panel.
    let mut unexplained = app.collector.current.correlation.clone();
    unexplained.cause = CorrelationCause::None;
    unexplained.lead = String::new();
    app.collector.current.correlation = unexplained;
    for width in [60, 30] {
        let buffer = chart_buffer(
            &app,
            width,
            10,
            "generation",
            &generation,
            ChartMetric::Generation,
        );
        let text = buffer_rows(&buffer).join("\n");
        assert!(text.contains("▼ nothing else unusual"), "{text}");
    }

    // Once the slowdown is over the line and the reason go; the samples
    // keep their colour in the history.
    app.collector.current.correlation = CorrelationInsight::default();
    let buffer = chart_buffer(
        &app,
        60,
        10,
        "generation",
        &generation,
        ChartMetric::Generation,
    );
    let text = buffer_rows(&buffer).join("\n");
    assert!(!text.contains('▼'), "{text}");
    assert!((0..60)
        .all(|x| (0..10).all(|y| buffer[(x, y)].symbol() != "┄" || buffer[(x, y)].fg != UNUSUAL)));
    assert!((53..59).all(|x| buffer[(x, 8)].fg == UNUSUAL));
}

#[test]
fn the_panel_behind_a_slowdown_is_tagged_in_the_anomaly_colour() {
    let mut app = test_app(0);
    app.collector.current = slowed_by_paging();
    let swap = defaults().swap_warn_rate;
    let paging_history = history_with_drop(0, swap * 2, Tone::Yellow);
    let buffer = chart_buffer(
        &app,
        60,
        10,
        "paging / I/O",
        &paging_history,
        ChartMetric::Swap,
    );
    let rows = buffer_rows(&buffer);
    assert!(rows[9].ends_with(" ▲ slowed GEN ↓50% ─┘"), "{}", rows[9]);
    // The border takes the anomaly colour; the summary keeps its own.
    assert_eq!(buffer[(0, 0)].fg, UNUSUAL);
    assert_eq!(buffer[(59, 5)].fg, UNUSUAL);
    assert!(rows[9].contains("page-in"), "{}", rows[9]);
    // A summary that would run into the tag gives way to it.
    let narrow = chart_buffer(
        &app,
        30,
        10,
        "paging / I/O",
        &paging_history,
        ChartMetric::Swap,
    );
    let bottom = buffer_rows(&narrow).pop().unwrap();
    assert!(
        bottom.ends_with(" ▼ GEN ─┘") && !bottom.contains("in "),
        "{bottom}"
    );
    // Other panels are left alone.
    let gpu = history_with_drop(99, 40, Tone::Green);
    let buffer = chart_buffer(&app, 60, 10, "GPU", &gpu, ChartMetric::Gpu);
    assert_ne!(buffer[(0, 0)].fg, UNUSUAL);

    // Each cause points at the panel that shows its signal.
    let panel = |cause: CorrelationCause, lead: &str| {
        let mut insight = app.collector.current.correlation.clone();
        insight.cause = cause;
        insight.lead = lead.into();
        crate::drop_marks::linked(&insight)
    };
    for (cause, lead, chart) in [
        (CorrelationCause::Paging, "paging", Some(Chart::Paging)),
        (CorrelationCause::Compression, "x", Some(Chart::Compression)),
        (CorrelationCause::MemoryPressure, "x", Some(Chart::Memory)),
        (CorrelationCause::ModelMemory, "x", Some(Chart::Memory)),
        (CorrelationCause::MetalMemory, "x", Some(Chart::Memory)),
        (CorrelationCause::Thermal, "x", Some(Chart::Gpu)),
        (CorrelationCause::GpuIdle, "x", Some(Chart::Gpu)),
        // The runtime's CPU is a figure in the header, not a chart.
        (CorrelationCause::Cpu, "x", None),
        (CorrelationCause::DiskIo, "x", Some(Chart::Cache)),
        (
            CorrelationCause::ConcurrentPrefill,
            "x",
            Some(Chart::Prefill),
        ),
        (CorrelationCause::Queueing, "x", Some(Chart::Queue)),
        (
            CorrelationCause::Stall,
            "no new token for 2s · disk read 1 GiB/s",
            Some(Chart::Cache),
        ),
        (
            CorrelationCause::Stall,
            "no new token for 2s · 3 SSD cache blocks loaded",
            Some(Chart::Cache),
        ),
        (
            CorrelationCause::Stall,
            "no new token for 2s · paging 80 MiB/s",
            Some(Chart::Paging),
        ),
        (
            CorrelationCause::Stall,
            "no new token for 2s · memory pressure",
            Some(Chart::Memory),
        ),
        (
            CorrelationCause::Stall,
            "no new token for 2s · GPU idle",
            None,
        ),
        // Nothing else unusual: nothing is tagged.
        (CorrelationCause::None, "", None),
    ] {
        assert!(panel(cause, lead) == chart, "{cause:?} {lead}");
    }
    let mut healthy = app.collector.current.correlation.clone();
    healthy.direction = ThroughputDirection::Flat;
    assert!(crate::drop_marks::linked(&healthy).is_none());

    // A stall is tagged as one; the queue and prompt panels carry tags too.
    app.collector.current.correlation.cause = CorrelationCause::Stall;
    app.collector.current.correlation.lead = "no new token for 2s · paging 80 MiB/s".into();
    let buffer = chart_buffer(
        &app,
        60,
        10,
        "paging / I/O",
        &paging_history,
        ChartMetric::Swap,
    );
    assert!(buffer_rows(&buffer)[9].ends_with(" ▲ stalled GEN ─┘"));
    app.collector.current.correlation.cause = CorrelationCause::Queueing;
    let queue = render_view(60, 8, |frame| {
        app.draw_operator_chart(frame, frame.area(), Chart::Queue)
    });
    assert!(queue.contains("slowed GEN"), "{queue}");
    // Too small to hold the tag: the panel is left as drawn.
    let tiny = render_view(8, 3, |frame| app.draw_request_chart(frame, frame.area()));
    assert!(!tiny.contains('▼') && !tiny.contains('▲'), "{tiny}");
}

// --- slowdowns: generation unusually slow, and what else is unusual -----

/// Ten polls at the usual 50 tok/s, then `rest`, observed in order.
fn after_usual(rest: Vec<Sample>) -> Vec<Sample> {
    let mut samples: Vec<Sample> = (0..10).map(|second| paging(second, 50.0, 0)).collect();
    samples.extend(rest);
    observe_all(&mut CorrelationEngine::default(), &mut samples);
    samples
}

#[test]
fn a_slowdown_is_three_slow_samples_and_ends_after_three_usual_ones() {
    let swap = defaults().swap_warn_rate * 2;
    let mut rest: Vec<Sample> = (10..20).map(|second| paging(second, 25.0, swap)).collect();
    rest.extend((20..25).map(|second| paging(second, 50.0, 0)));
    let samples = after_usual(rest);
    let reason = format!("paging {} (usual none)", rate(swap));

    // One slow poll, or two, is not a slowdown.
    assert!(!samples[10].correlation.is_material_drop());
    assert!(!samples[11].correlation.is_material_drop());
    let began = &samples[12].correlation;
    assert_eq!(began.direction, ThroughputDirection::Down);
    assert_eq!(began.cause, CorrelationCause::Paging);
    assert_eq!(
        began.summary,
        format!("GEN ↓50.0% (50.0→25.0 tok/s) · unusual: {reason}")
    );
    assert_eq!(began.lead, reason);
    assert_eq!(began.details, format!("paging · {reason}"));
    assert_eq!(began.unusual, vec![CorrelationCause::Paging]);
    assert!(began.slow);
    // It is counted from its first slow sample.
    assert_eq!(
        began.episode,
        Some(DropEpisode {
            reference: 50.0,
            low: 25.0,
            seconds: 2.0,
            samples: 3,
        })
    );
    // Its usual speed stays fixed however long it lasts.
    assert_eq!(samples[19].correlation.episode.unwrap().reference, 50.0);
    assert_eq!(samples[19].correlation.episode.unwrap().seconds, 9.0);

    // Two polls back at the usual speed do not end it; the third does.
    for index in [20, 21] {
        let insight = &samples[index].correlation;
        assert!(insight.is_material_drop(), "{index}");
        assert!(!insight.slow, "{index}: this sample is not itself slow");
        assert!(insight.summary.starts_with("GEN ↓50.0% (50.0→25.0 tok/s)"));
    }
    let over = &samples[22].correlation;
    assert_eq!(over.direction, ThroughputDirection::Flat);
    assert_eq!(
        over.ended,
        Some(DropEnd {
            settled: false,
            seconds: 12.0,
            low: 25.0,
            rate: 50.0,
            reference: 50.0,
        })
    );
    assert!(samples[23].correlation.ended.is_none());

    // The Journal: one row as it begins, one as it ends.
    assert_eq!(
        journal_rows(&samples, &["SLOWDOWN", "RECOVERED", "SETTLED"]),
        vec![
            (
                "SLOWDOWN".to_owned(),
                format!(
                    "GEN ↓50.0% (50.0→25.0 tok/s) · {reason} → Reduce active models; recheck paging."
                )
            ),
            (
                "RECOVERED".to_owned(),
                "generation back to 50.0 tok/s after 12s · low 25.0 tok/s".to_owned()
            ),
        ]
    );
}

#[test]
fn speed_that_jumps_about_is_usual_and_a_steady_runtime_shows_a_small_drop() {
    // The oMLX server's own log: speculative decoding swings between ~25
    // and ~120 tok/s from one second to the next. A run of slow polls is
    // what this runtime ordinarily does.
    let mut engine = CorrelationEngine::default();
    let jumpy = [
        56.0, 62.0, 67.0, 14.0, 30.0, 16.0, 25.0, 29.0, 79.0, 28.0, 28.0, 17.0, 34.0, 7.0, 52.0,
        62.0, 116.0, 105.0, 115.0, 117.0, 122.0, 50.0, 80.0, 72.0, 43.0, 48.0, 30.0, 21.0, 17.0,
        34.0, 19.0, 86.0, 119.0, 88.0, 109.0, 114.0, 81.0, 123.0, 45.0, 62.0, 80.0, 39.0, 17.0,
        32.0, 16.0,
    ];
    for (second, tps) in jumpy.iter().enumerate() {
        let insight = engine.observe(&paging(second as u64, *tps, 0), defaults());
        assert!(!insight.is_material_drop(), "{second}: {}", insight.summary);
    }
    // A runtime that holds 50 ± 1 tok/s: 38 is a slowdown, 44 is not (less
    // than a fifth below its usual speed).
    let steady = |slow: f64| {
        let mut samples: Vec<Sample> = (0..20)
            .map(|second| paging(second, 49.0 + (second % 3) as f64, 0))
            .collect();
        samples.extend((20..24).map(|second| paging(second, slow, 0)));
        observe_all(&mut CorrelationEngine::default(), &mut samples);
        samples.pop().unwrap().correlation
    };
    assert!(!steady(44.0).is_material_drop());
    let slowed = steady(38.0);
    assert!(slowed.is_material_drop());
    assert_eq!(
        slowed.summary,
        "GEN ↓24.0% (50.0→38.0 tok/s) · nothing else unusual"
    );
    assert_eq!(slowed.cause, CorrelationCause::None);
    assert!(slowed.lead.is_empty() && slowed.details.is_empty());
}

#[test]
fn a_slowdown_with_nothing_else_unusual_becomes_the_usual_speed_after_thirty_seconds() {
    let samples = after_usual((10..60).map(|second| paging(second, 30.0, 0)).collect());
    assert!(samples[41].correlation.is_material_drop());
    let settled = &samples[42].correlation;
    assert_eq!(settled.direction, ThroughputDirection::Flat);
    assert_eq!(
        settled.ended,
        Some(DropEnd {
            settled: true,
            seconds: 32.0,
            low: 30.0,
            rate: 30.0,
            reference: 50.0,
        })
    );
    // From then on thirty is the usual speed: nothing more is reported.
    assert!(
        samples[43..]
            .iter()
            .all(|sample| !sample.correlation.is_material_drop()
                && sample.correlation.ended.is_none())
    );
    assert_eq!(samples[59].correlation.delta_percent, Some(0.0));
    assert_eq!(
        journal_rows(&samples, &["SLOWDOWN", "RECOVERED", "SETTLED"]),
        vec![
            (
                "SLOWDOWN".to_owned(),
                "GEN ↓40.0% (50.0→30.0 tok/s) · nothing else unusual → Check the runtime's log at this time.".to_owned()
            ),
            (
                "SETTLED".to_owned(),
                "holding 30.0 tok/s for 32s, down from 50.0 · nothing else unusual; now the usual speed".to_owned()
            ),
        ]
    );
}

#[test]
fn a_signal_at_its_usual_level_is_not_the_reason_and_one_that_grew_says_from_what() {
    // Paging ran at the same rate while speed was usual: it is not what
    // changed when speed fell.
    let swap = defaults().swap_warn_rate * 2;
    let mut samples: Vec<Sample> = (0..10).map(|second| paging(second, 50.0, swap)).collect();
    samples.extend((10..14).map(|second| paging(second, 25.0, swap)));
    // Then it grew fourfold.
    samples.extend((14..16).map(|second| paging(second, 25.0, swap * 4)));
    observe_all(&mut CorrelationEngine::default(), &mut samples);
    let held = &samples[13].correlation;
    assert!(held.is_material_drop());
    assert_eq!(held.cause, CorrelationCause::None, "{}", held.summary);
    assert!(held.unusual.is_empty());
    let grew = &samples[15].correlation;
    assert_eq!(grew.cause, CorrelationCause::Paging);
    assert_eq!(
        grew.lead,
        format!("paging {} (usual {})", rate(swap * 4), rate(swap))
    );
}

/// A second request beside the one `decoding` builds.
fn beside(mut sample: Sample, prompt: u64, prefilling: bool) -> Sample {
    let mut other = sample.llm_requests[0].clone();
    other.id = "other".into();
    other.prompt = prompt;
    other.prefilled = prefilling.then_some(1_000);
    sample.llm_requests.push(other);
    sample
}

#[test]
fn each_signal_that_is_unusual_beside_a_slowdown_is_named_host_limits_first() {
    let compression = defaults().compression_warn_rate * 2;
    let usual = |second: u64| {
        let mut sample = paging(second, 50.0, 0);
        sample.pressure = "GREEN".into();
        sample.llm_waiting_requests = Some(0);
        sample.llm_model_memory = Some(80);
        sample.llm_model_memory_max = Some(100);
        sample.llm_count = 1;
        sample.llm_cpu = 95.0;
        sample
    };
    let slowed = |change: &dyn Fn(Sample) -> Sample| {
        let mut samples: Vec<Sample> = (0..10).map(usual).collect();
        samples.extend((10..13).map(|second| {
            let mut sample = usual(second);
            sample.llm_generation_tps = Some(25.0);
            change(sample)
        }));
        observe_all(&mut CorrelationEngine::default(), &mut samples);
        samples.pop().unwrap().correlation
    };
    type Change = Box<dyn Fn(Sample) -> Sample>;
    let cases: Vec<(CorrelationCause, String, Change)> = vec![
        (
            CorrelationCause::Thermal,
            "thermal limit".into(),
            Box::new(|mut s| {
                s.thermal = "limited 60%".into();
                s
            }),
        ),
        (
            CorrelationCause::MemoryPressure,
            "memory pressure critical".into(),
            Box::new(|mut s| {
                s.pressure = "RED".into();
                s
            }),
        ),
        (
            CorrelationCause::MemoryPressure,
            "memory pressure watch".into(),
            Box::new(|mut s| {
                s.pressure = "YELLOW".into();
                s
            }),
        ),
        (
            CorrelationCause::DiskIo,
            "disk read 900.0 MiB/s · 40 SSD cache blocks loaded (usual none)".into(),
            Box::new(|s| with_disk(s, 900 * MIB, 40)),
        ),
        (
            CorrelationCause::DiskIo,
            "1 SSD cache block saved (usual none)".into(),
            Box::new(|mut s| {
                s.disk.saves = 1;
                s
            }),
        ),
        (
            CorrelationCause::ConcurrentPrefill,
            "35.6k prefill beside generation".into(),
            Box::new(|s| beside(s, 35_628, true)),
        ),
        (
            CorrelationCause::Compression,
            format!("compression {} (usual none)", rate(compression)),
            Box::new(move |mut s| {
                s.compress = compression;
                s
            }),
        ),
        (
            CorrelationCause::Queueing,
            "2 waiting (usual 0)".into(),
            Box::new(|mut s| {
                s.llm_waiting_requests = Some(2);
                s
            }),
        ),
        (
            CorrelationCause::ModelMemory,
            "model MEM 95% of ceiling (usual 80%)".into(),
            Box::new(|mut s| {
                s.llm_model_memory = Some(95);
                s
            }),
        ),
        (
            CorrelationCause::MetalMemory,
            "Metal MEM 100% (usual 97%)".into(),
            Box::new(|mut s| {
                s.gpu_in_use = Some(100);
                s
            }),
        ),
        (
            CorrelationCause::Cpu,
            "runtime CPU 380% (usual 95%)".into(),
            Box::new(|mut s| {
                s.llm_cpu = 380.0;
                s
            }),
        ),
        (
            CorrelationCause::GpuIdle,
            "GPU 40% (usual 99%)".into(),
            Box::new(|mut s| {
                s.gpu_util = Some(40);
                s
            }),
        ),
    ];
    for (cause, lead, change) in &cases {
        let insight = slowed(change.as_ref());
        assert_eq!(insight.cause, *cause, "{}", insight.summary);
        assert_eq!(&insight.lead, lead);
        assert_eq!(insight.unusual, vec![*cause]);
        assert_eq!(insight.notes, vec![lead.clone()]);
        assert_eq!(insight.note(*cause), Some(lead.as_str()));
        // What it is judged against: slow from a fifth below the usual 50.
        assert_eq!(insight.usual_speed, Some((50.0, 40.0)));
        assert_eq!(
            insight.event_key,
            Some(CorrelationKey {
                direction: ThroughputDirection::Down,
                cause: *cause,
            })
        );
    }
    // A change short of mattering is not named: memory a point higher, a
    // prompt a few tokens longer, compression under its warning rate, a
    // little more CPU, a GPU still mostly busy.
    let quiet = slowed(&|mut s| {
        s.llm_cpu = 130.0;
        s.gpu_util = Some(85);
        s.llm_model_memory = Some(81);
        s.llm_requests[0].prompt += 500;
        s.compress = compression / 4;
        s
    });
    assert_eq!(quiet.cause, CorrelationCause::None, "{}", quiet.summary);

    // Several at once: the host's limits lead, the summary names two, the
    // details all of them.
    let both = slowed(&|mut s| {
        s.thermal = "limited 60%".into();
        s.llm_waiting_requests = Some(2);
        s
    });
    assert_eq!(
        both.note(CorrelationCause::Queueing),
        Some("2 waiting (usual 0)")
    );
    assert_eq!(both.note(CorrelationCause::Paging), None);
    // An idle GPU is what a slowdown looks like: it is named last.
    let starved = slowed(&|mut s| {
        s.gpu_util = Some(40);
        s.llm_cpu = 380.0;
        s
    });
    assert_eq!(
        starved.unusual,
        vec![CorrelationCause::Cpu, CorrelationCause::GpuIdle]
    );
    assert_eq!(starved.marked_samples(), 3, "known at its third sample");
    let mut later = starved.clone();
    later.episode.as_mut().unwrap().samples = 4;
    assert_eq!(later.marked_samples(), 1);
    assert_eq!(both.cause, CorrelationCause::Thermal);
    assert_eq!(
        both.summary,
        "GEN ↓50.0% (50.0→25.0 tok/s) · unusual: thermal limit + 2 waiting (usual 0)"
    );
    assert_eq!(
        both.details,
        "thermal limiting · thermal limit · 2 waiting (usual 0)"
    );
    assert_eq!(
        both.unusual,
        vec![CorrelationCause::Thermal, CorrelationCause::Queueing]
    );
}

#[test]
fn a_prompt_several_times_longer_is_new_work_and_is_learned_not_called_slow() {
    let long = |second: u64| {
        let mut sample = paging(second, 30.0, 0);
        sample.llm_requests[0].prompt = 203_379;
        sample
    };
    let samples = after_usual((10..30).map(long).collect());
    // Slower than the shorter prompt ran, and expected to be: it is never
    // a slowdown, and once seen it has a usual speed of its own.
    assert!(samples.iter().all(|s| !s.correlation.is_material_drop()));
    assert!(samples.iter().all(|s| !s.correlation.slow));
    assert_eq!(samples[17].correlation.usual_speed, None);
    let learned = &samples[29].correlation;
    assert_eq!(learned.usual_speed.unwrap().0, 30.0);
    assert!(
        learned.usual_for.starts_with("1 request · 205."),
        "{}",
        learned.usual_for
    );
}

#[test]
fn a_stall_inside_a_slowdown_is_journaled_once_and_the_slowdown_is_not_repeated() {
    let swap = defaults().swap_warn_rate * 2;
    let mut rest: Vec<Sample> = (10..14).map(|second| paging(second, 25.0, swap)).collect();
    // The request goes quiet, stalls for two polls, then resumes slow.
    let mut quiet = paging(14, 0.0, swap);
    quiet.llm_requests[0].idle_for = Some(Duration::from_secs_f64(0.9));
    rest.push(quiet);
    for (second, idle) in [(15, 2.0), (16, 3.1)] {
        let mut stalled = paging(second, 0.0, swap);
        stalled.llm_requests[0].idle_for = Some(Duration::from_secs_f64(idle));
        rest.push(stalled);
    }
    rest.extend((17..20).map(|second| paging(second, 25.0, swap)));
    let samples = after_usual(rest);
    // Going quiet is neither slow nor back to usual: the slowdown holds,
    // at the speed it had.
    assert!(samples[14].correlation.is_material_drop());
    assert!(!samples[14].correlation.slow);
    assert!(samples[14].correlation.summary.contains("50.0→25.0"));
    let stall = &samples[15].correlation;
    assert_eq!(stall.cause, CorrelationCause::Stall);
    assert!(stall.slow && stall.unusual.is_empty());
    assert_eq!(stall.summary, format!("GEN stalled · {}", stall.lead));
    assert_eq!(stall.episode.unwrap().samples, 6);
    assert!(samples[17].correlation.repeat);
    let rows = journal_rows(&samples, &["SLOWDOWN", "STALL", "RESUMED"]);
    assert_eq!(
        rows.iter()
            .map(|(state, _)| state.as_str())
            .collect::<Vec<_>>(),
        ["SLOWDOWN", "STALL", "RESUMED"]
    );
}

#[test]
fn a_slowdown_that_repeats_its_reason_within_a_minute_is_one_journal_story() {
    let swap = defaults().swap_warn_rate * 2;
    let burst = |from: u64| -> Vec<Sample> {
        (from..from + 5)
            .map(|second| paging(second, 25.0, swap))
            .chain((from + 5..from + 10).map(|second| paging(second, 50.0, 0)))
            .collect()
    };
    // Two bursts ten seconds apart, and a third two minutes later.
    let mut rest = burst(10);
    rest.extend(burst(20));
    rest.extend(burst(140));
    let samples = after_usual(rest);
    assert!(samples[22].correlation.is_material_drop());
    assert!(samples[22].correlation.repeat);
    assert!(
        samples[27].correlation.ended.is_none(),
        "told as part of the first: its end is not a row either"
    );
    let rows = journal_rows(&samples, &["SLOWDOWN", "RECOVERED"]);
    assert_eq!(
        rows.iter()
            .map(|(state, _)| state.as_str())
            .collect::<Vec<_>>(),
        ["SLOWDOWN", "RECOVERED", "SLOWDOWN", "RECOVERED"]
    );
}

#[test]
fn a_slowdown_waits_while_nothing_generates_and_is_over_after_thirty_idle_seconds() {
    let idle = |second: u64| {
        let mut sample = paging(second, 0.0, 0);
        sample.llm_generation_tps_live = false;
        sample.llm_requests.clear();
        sample.llm_active_requests = Some(0);
        sample
    };
    let swap = defaults().swap_warn_rate * 2;
    let samples = after_usual(vec![
        paging(10, 25.0, swap),
        paging(11, 25.0, swap),
        // Between two requests there is no speed to judge.
        idle(12),
        paging(13, 25.0, swap),
        paging(14, 25.0, swap),
        idle(15),
        paging(16, 25.0, swap),
        // Idle for longer than a slowdown takes to settle: it is over.
        idle(17),
        paging(50, 25.0, swap),
    ]);
    assert_eq!(
        samples[12].correlation.direction,
        ThroughputDirection::Unknown
    );
    let began = &samples[13].correlation;
    assert!(began.is_material_drop(), "three slow polls of generation");
    assert_eq!(began.episode.unwrap().seconds, 3.0);
    assert!(!samples[15].correlation.is_material_drop());
    assert!(samples[16].correlation.is_material_drop(), "it continues");
    assert_eq!(samples[16].correlation.episode.unwrap().samples, 5);
    assert!(samples[16].correlation.repeat);
    assert!(!samples[18].correlation.is_material_drop(), "a new start");
}

#[test]
fn usual_values_belong_to_one_model_and_remote_or_idle_rates_teach_nothing() {
    let mut engine = CorrelationEngine::default();
    for second in 0..10 {
        engine.observe(&paging(second, 50.0, 0), defaults());
    }
    assert_eq!(engine.speed.level(), Some(50.0));
    // A rate kept from the last request, with nothing running, is not one.
    let mut retained = paging(10, 0.0, 0);
    retained.llm_requests.clear();
    retained.llm_active_requests = Some(0);
    let insight = engine.observe(&retained, defaults());
    assert_eq!(insight.direction, ThroughputDirection::Unknown);
    assert!(insight.summary.is_empty());
    assert_eq!(engine.speed.level(), Some(50.0));
    // Another model starts from nothing.
    let mut other = paging(11, 20.0, 0);
    other.llm_model = "another".into();
    let insight = engine.observe(&other, defaults());
    assert_eq!(insight.direction, ThroughputDirection::Unknown);
    assert_eq!(insight.delta_percent, None);
    assert_eq!(engine.speed.level(), None);
    for second in 12..20 {
        let mut sample = paging(second, 20.0, 0);
        sample.llm_model = "another".into();
        engine.observe(&sample, defaults());
    }
    assert_eq!(engine.speed.level(), Some(20.0));
}

#[test]
fn a_slowdown_colours_its_samples_on_the_charts_of_what_was_unusual() {
    let mut collector = Collector::with_host(
        60,
        offline_config(),
        Box::new(FakeHost::default()),
        Platform::MacOs,
        None,
    );
    let fill = |history: &mut VecDeque<ChartPoint>, tone: Tone| {
        history.clear();
        history.extend((0..6).map(|_| ChartPoint::new(Some(10), tone)));
    };
    let tones = |history: &VecDeque<ChartPoint>| -> Vec<bool> {
        history
            .iter()
            .map(|point| point.tone == Tone::Unusual)
            .collect()
    };
    let reset = |collector: &mut Collector| {
        fill(&mut collector.generation_history, Tone::Green);
        fill(&mut collector.swap_history, Tone::Yellow);
        fill(&mut collector.compression_history, Tone::Green);
        fill(&mut collector.disk_history, Tone::Cyan);
        fill(&mut collector.load_history, Tone::Green);
        fill(&mut collector.gpu_history, Tone::Green);
        fill(&mut collector.prefill_history, Tone::Green);
        // A sample with no reading has nothing to colour.
        collector.swap_history[4].value = None;
    };
    reset(&mut collector);
    let mut sample = slowed_by_paging();

    // Known at its third slow sample: the two before it are marked too, on
    // the generation chart and on the chart of what was unusual.
    sample.correlation.episode.as_mut().unwrap().samples = 3;
    collector.mark_unusual(&sample);
    assert_eq!(
        tones(&collector.generation_history),
        [false, false, false, true, true, true]
    );
    assert_eq!(
        tones(&collector.swap_history),
        [false, false, false, true, false, true]
    );
    for history in [
        &collector.compression_history,
        &collector.disk_history,
        &collector.load_history,
        &collector.gpu_history,
        &collector.prefill_history,
    ] {
        assert!(tones(history).iter().all(|unusual| !unusual));
    }

    // After that, one sample at a time; a sample back at the usual speed
    // inside the slowdown keeps its own colour.
    reset(&mut collector);
    sample.correlation.episode.as_mut().unwrap().samples = 4;
    sample.correlation.slow = false;
    sample.correlation.unusual = vec![
        CorrelationCause::Compression,
        CorrelationCause::MemoryPressure,
        CorrelationCause::Thermal,
        CorrelationCause::ConcurrentPrefill,
        CorrelationCause::DiskIo,
        // Shown per request, not per sample: nothing to colour.
    ];
    collector.mark_unusual(&sample);
    assert!(tones(&collector.generation_history).iter().all(|u| !u));
    for history in [
        &collector.compression_history,
        &collector.disk_history,
        &collector.load_history,
        &collector.gpu_history,
        &collector.prefill_history,
    ] {
        assert_eq!(tones(history), [false, false, false, false, false, true]);
    }

    // A stall colours its own sample and the chart it points at.
    reset(&mut collector);
    sample.correlation = CorrelationInsight {
        direction: ThroughputDirection::Down,
        cause: CorrelationCause::Stall,
        lead: "no new token for 2s · disk read 900.0 MiB/s".into(),
        stall_seconds: Some(2.0),
        slow: true,
        ..CorrelationInsight::default()
    };
    collector.mark_unusual(&sample);
    assert!(tones(&collector.generation_history)[5]);
    assert!(tones(&collector.disk_history)[5]);
    assert!(!tones(&collector.generation_history)[4]);

    // Outside a slowdown an unusually slow sample and an unusual reading
    // are each coloured on their own chart, that one sample only.
    reset(&mut collector);
    sample.correlation = CorrelationInsight {
        direction: ThroughputDirection::Flat,
        slow: true,
        unusual: vec![CorrelationCause::GpuIdle],
        notes: vec!["GPU 40% (usual 99%)".into()],
        usual_speed: Some((50.0, 40.0)),
        ..CorrelationInsight::default()
    };
    collector.mark_unusual(&sample);
    let latest = [false, false, false, false, false, true];
    assert_eq!(tones(&collector.generation_history), latest);
    assert_eq!(tones(&collector.gpu_history), latest);
    assert!(tones(&collector.swap_history).iter().all(|u| !u));

    // A sample with nothing unusual in it touches nothing.
    reset(&mut collector);
    sample.correlation = CorrelationInsight::default();
    collector.mark_unusual(&sample);
    assert!(tones(&collector.generation_history).iter().all(|u| !u));
}

#[test]
fn outside_a_slowdown_a_sample_still_says_what_is_unusual_in_it() {
    let usual = |second: u64| {
        let mut sample = paging(second, 50.0, 0);
        sample.llm_waiting_requests = Some(0);
        sample.llm_count = 1;
        sample.llm_cpu = 95.0;
        sample
    };
    let mut engine = CorrelationEngine::default();
    let mut observe = |sample: Sample| engine.observe(&sample, defaults());
    for second in 0..10 {
        observe(usual(second));
    }

    // One slow sample is not a slowdown, and is unusually slow all the same.
    let mut slow = usual(10);
    slow.llm_generation_tps = Some(25.0);
    let lone = observe(slow);
    assert!(lone.slow && !lone.is_material_drop());
    assert_eq!(lone.usual_speed, Some((50.0, 40.0)));
    assert!(lone.unusual.is_empty() && lone.summary.is_empty());
    assert_eq!(lone.marked_samples(), 1);

    // A reading of the host far from its usual value is unusual while
    // generation runs at its usual speed, and says what is usual.
    let mut busy = usual(11);
    busy.llm_cpu = 380.0;
    busy.gpu_util = Some(40);
    let found = observe(busy);
    assert!(!found.slow && !found.is_material_drop());
    assert_eq!(
        found.unusual,
        vec![CorrelationCause::Cpu, CorrelationCause::GpuIdle]
    );
    assert_eq!(
        found.note(CorrelationCause::Cpu),
        Some("runtime CPU 380% (usual 95%)")
    );
    assert_eq!(
        found.note(CorrelationCause::GpuIdle),
        Some("GPU 40% (usual 99%)")
    );
    // Nothing for the Journal: that is for slowdowns.
    assert!(found.summary.is_empty() && found.event_key.is_none());

    // The workload is not watched: another request and a queue are what
    // was asked for, and only ever the reason for a slowdown.
    let mut asked = beside(usual(12), 9_000, true);
    asked.llm_waiting_requests = Some(2);
    assert!(observe(asked).unusual.is_empty());

    // A burst that came and went leaves the usual value where it was. One
    // that holds for thirty seconds at the usual speed is the usual value
    // from then on.
    for second in 13..=44 {
        let mut sample = usual(second);
        sample.llm_cpu = 380.0;
        let insight = observe(sample);
        assert_eq!(
            insight.unusual,
            if second <= 43 {
                vec![CorrelationCause::Cpu]
            } else {
                Vec::new()
            },
            "second {second}"
        );
    }

    // With nothing generating there is no speed to judge, and the usual
    // speed is still known.
    let mut idle = usual(45);
    idle.llm_generation_tps = None;
    idle.llm_generation_tps_live = false;
    idle.llm_active_requests = Some(0);
    idle.llm_requests.clear();
    let idle = observe(idle);
    assert_eq!(idle.direction, ThroughputDirection::Unknown);
    assert_eq!(idle.usual_speed, Some((50.0, 40.0)));
    assert!(!idle.slow && idle.unusual.is_empty());
}

#[test]
fn the_generation_chart_always_draws_its_usual_speed_quietly() {
    let mut app = test_app(0);
    let mut sample = paging(30, 50.0, 0);
    sample.correlation = CorrelationInsight {
        direction: ThroughputDirection::Flat,
        usual_speed: Some((50.0, 40.0)),
        ..CorrelationInsight::default()
    };
    app.collector.current = sample;
    // Sixteen samples at the usual speed, one of them unusually slow.
    let mut generation = history_with_drop(500, 500, Tone::Green);
    generation[12] = ChartPoint::new(Some(200), Tone::Unusual);
    let buffer = chart_buffer(
        &app,
        60,
        10,
        "generation",
        &generation,
        ChartMetric::Generation,
    );
    let rows = buffer_rows(&buffer);
    let text = rows.join("\n");
    // The dashed line sits at the usual speed, with its value on the axis,
    // and is quiet: nothing is wrong.
    let usual = rows
        .iter()
        .position(|row| row.contains('┄'))
        .expect("a line at the usual speed") as u16;
    assert!(
        rows[usize::from(usual)]
            .trim_start_matches('│')
            .trim_start()
            .starts_with("50 "),
        "{text}"
    );
    let dashes: Vec<u16> = (1..59)
        .filter(|x| buffer[(*x, usual)].symbol() == "┄")
        .collect();
    assert!(dashes.contains(&55), "{dashes:?}\n{text}");
    assert!(dashes
        .iter()
        .all(|x| buffer[(*x, usual)].fg == Tone::Muted.color()));
    // The border says what the line is. No reason is given: nothing slowed.
    assert!(rows[9].contains("tok/s · ┄ usual 50.0"), "{text}");
    assert!(!text.contains('▼'), "{text}");
    // The slow sample alone is in the anomaly colour.
    assert_eq!(buffer[(55, 8)].fg, UNUSUAL);
    assert!((43..59)
        .filter(|x| *x != 55)
        .all(|x| buffer[(x, 8)].fg == GREEN));

    // A narrow panel keeps the unit and lets the usual value go.
    let narrow = buffer_rows(&chart_buffer(
        &app,
        22,
        10,
        "generation",
        &generation,
        ChartMetric::Generation,
    ));
    assert!(narrow[9].contains("tok/s"), "{}", narrow[9]);
    assert!(!narrow[9].contains("usual"), "{}", narrow[9]);

    // Before a usual speed is known there is nothing to say about it.
    app.collector.current.correlation = CorrelationInsight::default();
    let unknown = buffer_rows(&chart_buffer(
        &app,
        60,
        10,
        "generation",
        &generation,
        ChartMetric::Generation,
    ));
    assert!(!unknown.join("\n").contains("usual"));
}

#[test]
fn dips_that_come_and_go_become_part_of_the_usual_range() {
    // Two slow polls and back, again and again: never three in a row, so
    // never a slowdown, and each dip is learned as something this runtime
    // does. Before long three in a row are ordinary too.
    let mut engine = CorrelationEngine::default();
    let mut second = 0;
    let mut observe = |engine: &mut CorrelationEngine, tps: f64| {
        second += 1;
        engine.observe(&paging(second, tps, 0), defaults())
    };
    for _ in 0..10 {
        observe(&mut engine, 50.0);
    }
    for _ in 0..12 {
        for tps in [25.0, 25.0, 50.0] {
            assert!(!observe(&mut engine, tps).is_material_drop());
        }
    }
    for _ in 0..4 {
        let insight = observe(&mut engine, 25.0);
        assert!(!insight.is_material_drop(), "{}", insight.summary);
    }
}

#[test]
fn a_stall_names_everything_the_host_was_doing_beside_it() {
    let mut stalled = beside(paging(3, 0.0, PAGING_ACTIVE_ENTER_RATE), 9_000, false);
    for request in &mut stalled.llm_requests {
        request.idle_for = Some(Duration::from_secs_f64(2.2));
    }
    stalled.gpu_util = Some(40);
    stalled.compress = defaults().compression_warn_rate;
    stalled.pressure = "YELLOW".into();
    stalled.disk = domain::DiskActivity {
        write: Some(64 * MIB),
        ..Default::default()
    };
    let insight = CorrelationEngine::default().observe(&stalled, defaults());
    assert_eq!(insight.cause, CorrelationCause::Stall);
    assert_eq!(
        insight.lead,
        format!(
            "no new token for 2s · GPU 40% · disk write 64.0 MiB/s · paging {} · compression {} · memory pressure · 2 requests running",
            rate(PAGING_ACTIVE_ENTER_RATE),
            rate(defaults().compression_warn_rate)
        )
    );
    // No usual speed is needed to see a stall, and none is learned from it.
    assert!(insight.episode.is_none());
}

#[test]
fn diagnostics_and_the_expanded_chart_say_what_a_slowdown_is_judged_against() {
    let mut sample = slowed_by_paging();
    sample.llm_observed_at = Some(SystemTime::now());
    sample.correlation.notes = vec![
        "paging 2.0 MiB/s (usual none)".into(),
        "GPU 40% (usual 99%)".into(),
    ];
    sample.correlation.unusual = vec![CorrelationCause::Paging, CorrelationCause::GpuIdle];
    sample.correlation.usual_speed = Some((50.0, 40.0));
    let lines = crate::report::diagnostic_lines(&sample);
    let at = |text: &str| lines.iter().position(|line| line == text);
    assert!(at("Usual generation speed 50.0 tok/s · slow below 40.0").is_some());
    assert!(
        at("Unusual: paging 2.0 MiB/s (usual none)") < at("Unusual: GPU 40% (usual 99%)"),
        "{lines:?}"
    );
    assert!(at("Unusual: GPU 40% (usual 99%)") < at("CONNECTION"));
    let mut report = Vec::new();
    crate::report::write_static(&mut report, &sample, 2, defaults(), Platform::MacOs).unwrap();
    let report = String::from_utf8(report).unwrap();
    assert!(
        report.contains("USUAL SPEED   50.0 tok/s · slow below 40.0"),
        "{report}"
    );

    // An expanded generation chart names the same two figures in its title.
    let mut app = test_app(0);
    app.collector.current = sample.clone();
    app.charts.expanded = true;
    let generation = history_with_drop(500, 250, Tone::Unusual);
    let top = buffer_rows(&chart_buffer(
        &app,
        140,
        10,
        "generation",
        &generation,
        ChartMetric::Generation,
    ))
    .remove(0);
    assert!(top.contains("usual 50.0 · slow below 40.0"), "{top}");
    assert!(top.contains("slow 12s · low 25.0"), "{top}");
    // No other chart claims it, and neither does the Overview's compact one.
    let paging_chart = buffer_rows(&chart_buffer(
        &app,
        110,
        10,
        "paging / I/O",
        &generation,
        ChartMetric::Swap,
    ))
    .remove(0);
    assert!(!paging_chart.contains("usual"), "{paging_chart}");
    app.charts.expanded = false;
    let compact = buffer_rows(&chart_buffer(
        &app,
        110,
        10,
        "generation",
        &generation,
        ChartMetric::Generation,
    ))
    .remove(0);
    assert!(!compact.contains("usual"), "{compact}");

    // Outside a slowdown the figures stay, and a reading that is unusual
    // on its own is still listed, here and in the one-shot report.
    sample.correlation = CorrelationInsight {
        direction: ThroughputDirection::Flat,
        unusual: vec![CorrelationCause::GpuIdle],
        notes: vec!["GPU 40% (usual 99%)".into()],
        usual_speed: Some((50.0, 40.0)),
        ..CorrelationInsight::default()
    };
    let lines = crate::report::diagnostic_lines(&sample);
    assert!(lines
        .iter()
        .any(|line| line.starts_with("Usual generation speed")));
    assert!(lines.contains(&"Unusual: GPU 40% (usual 99%)".to_owned()));
    let mut report = Vec::new();
    crate::report::write_static(&mut report, &sample, 2, defaults(), Platform::MacOs).unwrap();
    let report = String::from_utf8(report).unwrap();
    assert!(
        report.contains("UNUSUAL       GPU 40% (usual 99%)"),
        "{report}"
    );
    assert!(!report.contains("SLOWDOWN"), "{report}");

    // With nothing unusual, nothing is listed.
    sample.correlation.unusual.clear();
    sample.correlation.notes.clear();
    let lines = crate::report::diagnostic_lines(&sample);
    assert!(!lines.iter().any(|line| line.starts_with("Unusual:")));
}

#[test]
fn speed_is_judged_against_earlier_samples_of_similar_work() {
    let mut engine = CorrelationEngine::default();
    let mut observe = |sample: Sample| engine.observe(&sample, defaults());
    let alone = |second: u64, tps: f64| paging(second, tps, 0);
    let pair = |second: u64, tps: f64| beside(paging(second, tps, 0), 60_000, false);
    let long = |second: u64, tps: f64| {
        let mut sample = paging(second, tps, 0);
        sample.llm_requests[0].prompt = 240_000;
        sample
    };
    for second in 0..10 {
        observe(alone(second, 50.0));
    }
    // Two requests writing at once, never seen before: there is nothing
    // like it to compare with, so it is learned rather than called slow
    // against a lone request's speed.
    let first = observe(pair(10, 30.0));
    assert!(!first.slow && !first.is_material_drop());
    assert_eq!(first.usual_speed, None);

    // Once two requests have run at that speed it is their usual speed,
    // and a lone request is still judged against lone ones.
    for second in 11..30 {
        observe(pair(second, 30.0));
    }
    let two = observe(pair(30, 30.0));
    assert!(!two.slow && two.unusual.is_empty(), "{}", two.summary);
    assert_eq!(two.usual_speed.unwrap().0, 30.0);
    assert_eq!(two.usual_for, "2 requests · 131.2k context");
    let one = observe(alone(31, 50.0));
    assert!(!one.slow);
    assert_eq!(one.usual_speed.unwrap().0, 50.0);
    assert_eq!(one.usual_for, "1 request · 69.5k context");

    // The same holds for a context several times longer: slower, and
    // usual for that context, while the shorter one keeps its own speed.
    for second in 32..50 {
        observe(long(second, 36.0));
    }
    let longer = observe(long(50, 36.0));
    assert!(!longer.slow, "{}", longer.summary);
    assert_eq!(longer.usual_speed.unwrap().0, 36.0);
    assert_eq!(longer.usual_for, "1 request · 242.5k context");
    let short = observe(alone(51, 36.0));
    assert!(short.slow, "36 tok/s is slow for the shorter context");
    assert_eq!(short.usual_speed.unwrap().0, 50.0);

    // Three slow samples of familiar work are a slowdown, and the request
    // count, already allowed for, is not given as its reason.
    let slowdown = (52..55)
        .map(|second| observe(pair(second, 12.0)))
        .last()
        .unwrap();
    assert!(slowdown.is_material_drop());
    assert_eq!(
        slowdown.cause,
        CorrelationCause::None,
        "{}",
        slowdown.summary
    );
    assert_eq!(
        slowdown.summary,
        "GEN ↓60.0% (30.0→12.0 tok/s) · nothing else unusual"
    );
}

#[test]
fn the_usual_speed_line_follows_the_work_column_by_column() {
    let mut app = test_app(0);
    let mut sample = paging(30, 30.0, 0);
    sample.correlation = CorrelationInsight {
        direction: ThroughputDirection::Flat,
        usual_speed: Some((30.0, 24.0)),
        usual_for: "2 requests · 128.5k context".into(),
        ..CorrelationInsight::default()
    };
    app.collector.current = sample;
    // Eight samples alone at a usual 50 tok/s, then eight beside another
    // request at a usual 30: every bar is at its own usual speed.
    let mut generation: VecDeque<ChartPoint> = (0..16)
        .map(|index| {
            let usual = if index < 8 { 500 } else { 300 };
            let mut point = ChartPoint::new(Some(usual * 4 / 5), Tone::Green);
            point.usual = Some(usual);
            point
        })
        .collect();
    let buffer = chart_buffer(
        &app,
        68,
        12,
        "generation",
        &generation,
        ChartMetric::Generation,
    );
    let rows = buffer_rows(&buffer);
    let text = rows.join("\n");
    // Each column's line row, left to right.
    let lines: Vec<(u16, u16)> = (1..67)
        .filter_map(|x| {
            (1..11)
                .find(|y| buffer[(x, *y)].symbol() == "┄")
                .map(|y| (x, y))
        })
        .collect();
    let rows_seen: Vec<u16> = lines.iter().map(|(_, y)| *y).collect();
    // The line steps down once, where the work got heavier, and is grey:
    // nothing is unusual about a speed that matches its work.
    let (first, last) = (lines[0], lines[lines.len() - 1]);
    assert!(first.1 < last.1, "{lines:?}\n{text}");
    assert!(
        rows_seen.windows(2).all(|pair| pair[0] <= pair[1]),
        "{text}"
    );
    assert_eq!(buffer[last].fg, Tone::Muted.color());
    // The border names the work the latest usual speed is for.
    assert!(
        rows[11].contains("tok/s · ┄ usual 30.0 for 2 requests · 128.5k context"),
        "{text}"
    );

    // Bars past their usual speed have the line cut through them, so it
    // reads across the whole chart.
    let tall: VecDeque<ChartPoint> = generation
        .iter()
        .map(|point| ChartPoint {
            value: point.usual.map(|usual| usual * 6 / 5),
            ..*point
        })
        .collect();
    let crossed = chart_buffer(&app, 68, 12, "generation", &tall, ChartMetric::Generation);
    let through: Vec<u16> = (1..67)
        .filter(|x| {
            (1..11).any(|y| {
                let cell = &crossed[(*x, y)];
                cell.symbol() == "┄" && cell.bg == GREEN && cell.fg == PANEL
            })
        })
        .collect();
    assert!(
        through.len() >= 16,
        "{through:?}\n{}",
        buffer_rows(&crossed).join("\n")
    );

    // A slow sample shows as a bar short of its own column's line, and the
    // line over it takes the anomaly colour.
    let newest = generation.back_mut().unwrap();
    newest.value = Some(100);
    newest.tone = Tone::Unusual;
    let buffer = chart_buffer(
        &app,
        68,
        12,
        "generation",
        &generation,
        ChartMetric::Generation,
    );
    assert_eq!(
        buffer[last].fg,
        UNUSUAL,
        "{}",
        buffer_rows(&buffer).join("\n")
    );
}

#[test]
fn decode_time_reads_are_weights_and_the_disk_panel_shows_their_cost() {
    use crate::collector::weight_reads;
    let mut sample = Sample {
        llm_generation_tps: Some(18.0),
        disk: domain::DiskActivity {
            read: Some(900 * MIB),
            write: Some(0),
            ..Default::default()
        },
        ..Sample::default()
    };
    assert_eq!(
        weight_reads(&sample),
        900 * MIB,
        "reads while decoding are weights"
    );
    sample.disk.loads = 3;
    assert_eq!(weight_reads(&sample), 0, "a prompt-cache load is the cache");
    sample.disk.loads = 0;
    sample.llm_prefill_tokens_expected = Some(4000);
    assert_eq!(weight_reads(&sample), 0, "prefill reads its cache");
    sample.llm_prefill_tokens_expected = None;
    sample.disk.read = Some(MIB);
    assert_eq!(weight_reads(&sample), 0, "a trickle is not streaming");
    sample.disk.read = Some(900 * MIB);
    sample.llm_generation_tps = None;
    assert_eq!(weight_reads(&sample), 0, "no decode, no weights");

    let mut reads = VecDeque::new();
    assert_eq!(crate::cache_impact::weights(&reads), None);
    for (speed, from_disk) in [(42.0, false), (40.0, false), (18.0, true), (44.0, false)] {
        reads.push_back(domain::DecodeRead { speed, from_disk });
    }
    assert_eq!(
        crate::cache_impact::weights(&reads),
        Some((0.25, Some(18.0), Some(42.0)))
    );

    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    app.collector.decode_reads = reads;
    app.collector.current.disk = domain::DiskActivity {
        read: Some(900 * MIB),
        write: Some(0),
        weights: 900 * MIB,
        ..Default::default()
    };
    for second in 0..20 {
        push_history_with_tone(
            &mut app.collector.disk_history,
            Some(if second % 2 == 0 { 900 * MIB } else { 0 }),
            Tone::Blue,
            60,
        );
    }
    let (name, history, metric) = app.cache_panel();
    let history = history.clone();
    let screen = render_view(80, 10, |frame| {
        app.render_indicator_chart(frame, frame.area(), name, &history, metric)
    });
    for label in [
        "disk I/O",
        "streaming weights",
        "weights",
        "25% of decode from disk · 18 vs 42 tok/s",
        "streaming weights from disk",
    ] {
        assert!(screen.contains(label), "missing {label}\n{screen}");
    }
    let narrow = render_view(30, 10, |frame| {
        app.render_indicator_chart(frame, frame.area(), name, &history, metric)
    });
    assert!(narrow.contains("weights"), "{narrow}");
}
