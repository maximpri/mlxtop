// SPDX-License-Identifier: MIT
//! The Overview and its panels: what each draws and says.
use super::*;

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
            "17.2 / 36.0 GiB",
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
    for label in ["3.2 / 27.0 GiB", "17.2 / 36.0 GiB"] {
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
fn llm_top_sort_cycles_in_a_predictable_order() {
    assert_eq!(TopSort::Rss.next(), TopSort::Cpu);
    assert_eq!(TopSort::Cpu.next(), TopSort::Pid);
    assert_eq!(TopSort::Pid.next(), TopSort::Name);
    assert_eq!(TopSort::Name.next(), TopSort::Rss);
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
fn log_rates_are_labeled_as_last_results() {
    let sample = Sample {
        llm_source: TelemetrySource::Log,
        llm_generation_tps: Some(28.5),
        ..Sample::default()
    };

    assert_eq!(llm_generation_rate_label(&sample), "LAST GEN 28.5 tok/s");
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
fn host_row_is_memory_cache_and_paging_and_compression_sits_above_the_queue() {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    let screen = render_app(&app, 120, 40);
    for label in [
        "PRESSURE normal",
        "█wired █app █comp █cache",
        "18.8 GiB left",
    ] {
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
fn memory_shows_headroom_against_the_gpu_limit_and_what_ram_has_left() {
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
    {
        let sample = &mut app.collector.current;
        sample.total_memory = 36 * gib;
        sample.wired = 20 * gib;
        sample.anonymous = 6 * gib;
        sample.compressor = 2 * gib;
        sample.file_backed = 4 * gib;
        sample.metal.resource_limit = Some(32 * gib);
        sample.pressure_tone = Tone::Yellow;
    }
    let draw = |app: &App, width: u16, height: u16| {
        let mut terminal =
            Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
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
        terminal.backend().buffer().clone()
    };
    let buffer = draw(&app, 60, 20);
    let text: String = (0..20)
        .map(|y| (0..60).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n")
        .collect();
    for label in [
        "GPU",
        "20.0 / 32.0 GiB",
        "wired, against the GPU limit",
        "12.0 GiB left",
        "RAM",
        "28.0 / 36.0 GiB",
        "█wired █app █comp █cache",
        "8.0 GiB left",
        "18.0 GiB",
    ] {
        assert!(text.contains(label), "missing {label}\n{text}");
    }
    assert!(!text.contains("resident"), "{text}");
    // The GPU meter fills to wired over the limit.
    let filled = (0..60).filter(|x| buffer[(*x, 1)].bg == CYAN).count();
    let track = (0..60).filter(|x| buffer[(*x, 1)].bg == EDGE).count();
    assert!(
        filled > track,
        "20 of 32 GiB: {filled} filled, {track} empty"
    );
    // What was left: 18 GiB before the load, 8 GiB after, under its pressure.
    let height = |x: u16| {
        (0..20)
            .filter(|y| "▁▂▃▄▅▆▇█".contains(buffer[(x, *y)].symbol()))
            .count()
    };
    assert!(
        height(30) > height(57) + 2,
        "{} vs {}",
        height(30),
        height(57)
    );
    assert_eq!(buffer[(57, 18)].fg, YELLOW);
    assert_eq!(buffer[(30, 18)].fg, GREEN);

    // Wired past the limit says so, in red.
    app.collector.current.wired = 33 * gib;
    let buffer = draw(&app, 60, 20);
    let text: String = (0..20)
        .map(|y| (0..60).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n")
        .collect();
    assert!(text.contains("wired over the GPU limit"), "{text}");
    assert!(text.contains("0 B left"), "{text}");
    assert_eq!(buffer[(6, 1)].bg, RED);

    // No GPU limit known: RAM alone. A short panel keeps the meters only.
    app.collector.current.metal.resource_limit = None;
    app.collector.current.mlx.recommended_working_set = None;
    let buffer = draw(&app, 40, 4);
    let text: String = (0..4)
        .map(|y| (0..40).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n")
        .collect();
    assert!(text.contains("RAM") && !text.contains("GPU"), "{text}");

    // Without composition counters (Linux), memory stays an area chart.
    app.collector.current.vm_available = false;
    app.collector.composition_history = std::iter::repeat_n(None, 40).collect();
    let buffer = draw(&app, 60, 20);
    assert!((0..20).any(|y| buffer[(57, y)].symbol() == "━"));
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

#[test]
fn the_cache_panel_shows_what_the_cache_saved_over_its_disk_traffic() {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    // Without disk readings the panel keeps each interval's reuse.
    assert_eq!(app.cache_panel().0, "KV cache");
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
    // Of those, mlxtop watched 360k being served from cache.
    app.collector.current.llm_watched_cached_tokens = Some(360_000);
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
        "KV cache / model I/O",
        "KV hit 90% · ≈30m 0s saved",
        "session",
        "90% of 1.2M",
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
    assert!(screen.contains("KV cache"), "{screen}");
    app.charts.focused = Chart::Cache;
    app.charts.expanded = true;
    assert!(render_app(&app, 120, 40).contains("KV cache"));
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
        "KV hit 60%",
        "nothing watched yet"
    );
    sample.llm_watched_cached_tokens = Some(600);
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
        )
    });
    assert!(bar.contains("RAM 60% · SSD <1%"), "{bar}");
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
        "KV cache / model I/O",
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

#[test]
fn u_opens_the_session_summary_and_any_key_closes_it() {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
    assert!(app.summary_open);
    let screen = render_app(&app, 120, 40);
    assert!(screen.contains("Session summary"), "{screen}");
    assert!(screen.contains("Speed lost: none"), "{screen}");
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert!(!app.summary_open && !app.quit);
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(app.quit);
    app.help = true;
    assert!(render_app(&app, 120, 40).contains("u               session summary"));
}
