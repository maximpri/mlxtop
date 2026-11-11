// SPDX-License-Identifier: MIT
//! Finding slowdowns and stalls: usual speeds, similar work, reasons.
use super::*;

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
fn a_slowdown_keeps_its_reason_after_the_reason_passes() {
    // The server, 11 Nov 17:25: a new prompt's prefill and disk writes
    // slowed an answer, then ended while the answer was still slow. The
    // headline said nothing explained it; the slowdown is still theirs.
    let thresholds = defaults();
    let mut engine = CorrelationEngine::default();
    let mut last = CorrelationInsight::default();
    for second in 0..30u64 {
        let slow = second >= 20;
        let tps = if slow {
            15.0
        } else {
            CAPTURED_SPEEDS[second as usize % 17]
        };
        let mut sample = decoding(second, tps, 1_000 + second * 40, 0.05, 99);
        sample.rate_ready = true;
        sample.swap_in = if (20..24).contains(&second) {
            thresholds.swap_warn_rate * 4
        } else {
            0
        };
        last = engine.observe(&sample, thresholds);
        if second == 23 {
            assert_eq!(last.cause, CorrelationCause::Paging, "{}", last.summary);
        }
    }
    assert!(last.is_material_drop(), "{}", last.summary);
    assert_eq!(last.cause, CorrelationCause::Paging, "{}", last.summary);
    assert!(last.lead.ends_with(" earlier"), "{}", last.lead);
    assert!(
        !last.summary.contains("nothing else unusual"),
        "{}",
        last.summary
    );
    assert!(last.unusual.is_empty(), "nothing is unusual in this sample");
}

#[test]
fn a_stall_does_not_blame_disk_writes_that_ran_as_fast_as_ever() {
    // The server, 11 Nov 17:02:44–17:03:06: two requests wrote ~38 MiB/s to
    // disk at 60–85 tok/s; then for seven seconds no token came, the GPU
    // idle, the writes unchanged; then 80 tok/s again. The writes ran the
    // same while it was fast, so they are not why it stopped.
    let mib = 1024 * 1024;
    let mut engine = CorrelationEngine::default();
    let mut out = 1_000;
    let mut stalled = None;
    // (tok/s, disk write MiB/s, GPU %), one second each.
    let mut seconds: Vec<(f64, u64, u8)> = Vec::new();
    for _ in 0..3 {
        seconds.extend([(58.0, 39, 99), (68.0, 34, 99), (52.0, 0, 99), (50.0, 0, 99)]);
    }
    seconds.extend([(77.0, 1, 99), (44.0, 20, 99)]);
    seconds.extend([(0.0, 37, 0); 6]);
    seconds.extend([(86.0, 38, 99), (75.0, 31, 99)]);
    let mut idle = 0.0;
    for (second, (tps, write, gpu)) in seconds.into_iter().enumerate() {
        if tps > 0.0 {
            out += tps as u64;
            idle = 0.05;
        } else {
            idle += 1.0;
        }
        let mut sample = decoding(second as u64, tps, out, idle, gpu);
        sample.disk.write = Some(write * mib);
        sample.disk.read = Some(0);
        let insight = engine.observe(&sample, defaults());
        if insight.cause == CorrelationCause::Stall && insight.is_material_drop() {
            stalled.get_or_insert(insight);
        }
    }
    let stall = stalled.expect("the pause is a stall");
    assert!(stall.unusual.is_empty(), "{:?}", stall.unusual);
    assert!(
        stall
            .lead
            .contains("GPU idle · as when it ran fast: disk write"),
        "{}",
        stall.lead
    );
    assert!(crate::drop_marks::linked(&stall).is_none());

    // Writes it had not done while fast are still named.
    let mut engine = CorrelationEngine::default();
    let mut stall = None;
    for second in 0..20u64 {
        let pause = second >= 14;
        let tps = if pause { 0.0 } else { 50.0 };
        let mut sample = decoding(
            second,
            tps,
            1_000 + second.min(14) * 50,
            if pause { (second - 13) as f64 } else { 0.05 },
            if pause { 0 } else { 99 },
        );
        sample.disk.write = Some(if pause { 400 * mib } else { 0 });
        sample.disk.read = Some(0);
        let insight = engine.observe(&sample, defaults());
        if insight.cause == CorrelationCause::Stall && insight.is_material_drop() {
            stall.get_or_insert(insight);
        }
    }
    let stall = stall.expect("the pause is a stall");
    assert_eq!(stall.unusual, vec![CorrelationCause::DiskIo]);
    assert_eq!(crate::drop_marks::linked(&stall), Some(Chart::Cache));
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
                "no new token for 2s at 1.7k out · GPU idle → The runtime paused with the GPU idle and nothing on this machine out of the ordinary: the cause is inside it; check its log at this time.".to_owned()
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
    assert!(crate::diagnosis::slowdown_next_step(&samples[18])
        .starts_with("The SSD prompt cache is evicting"));
    let mut not_full = samples[18].clone();
    not_full.correlation.lead = "no new token for 2s · disk read 850.0 MiB/s".into();
    assert_eq!(
        crate::diagnosis::slowdown_next_step(&not_full),
        format!(
            "The runtime waited on its disk ({}): put its cache on a faster disk.",
            not_full.correlation.lead
        )
    );
    not_full.correlation.cause = CorrelationCause::DiskIo;
    not_full.correlation.lead = "1 SSD cache block saved".into();
    assert_eq!(
        crate::diagnosis::slowdown_next_step(&not_full),
        format!(
            "The runtime waited on its disk ({}): put its cache on a faster disk.",
            not_full.correlation.lead
        )
    );
    not_full.correlation.lead = "x".into();
    assert!(crate::diagnosis::slowdown_next_step(&not_full)
        .starts_with("The runtime used its disk mid-answer (x)"));
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
    assert!(rows[9].contains("swapping: memory is short"), "{}", rows[9]);
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
        // A stall names what ran differently from when the model was fast,
        // as the engine does; readings as usual name nothing.
        if cause == CorrelationCause::Stall {
            let named = lead.split(" · as when").next().unwrap_or("");
            insight.unusual = [
                ("disk ", CorrelationCause::DiskIo),
                ("SSD cache", CorrelationCause::DiskIo),
                ("paging", CorrelationCause::Paging),
                ("memory pressure", CorrelationCause::MemoryPressure),
            ]
            .into_iter()
            .filter(|(word, _)| named.contains(word))
            .map(|(_, cause)| cause)
            .collect();
        }
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
        // Disk writes at the rate they ran while the model was fast are
        // not the reason it stopped: nothing is tagged.
        (
            CorrelationCause::Stall,
            "no new token for 2s · GPU idle · as when it ran fast: disk write 38.0 MiB/s",
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
                    "GEN ↓50.0% (50.0→25.0 tok/s) · {reason} → The model no longer fits in RAM: close other apps or models, or use a smaller quant."
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
                "GEN ↓40.0% (50.0→30.0 tok/s) · nothing else unusual → Nothing on the machine explains it: the text, the draft model or the runtime itself; check the runtime's log at this time.".to_owned()
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
    // The first poll after a pause covers part of an interval: not judged.
    assert!(!samples[13].correlation.is_material_drop());
    assert!(
        !samples[13].correlation.slow,
        "a partial interval is never slow"
    );
    let began = &samples[14].correlation;
    assert!(
        began.is_material_drop(),
        "three whole slow polls of generation"
    );
    assert_eq!(began.episode.unwrap().seconds, 4.0);
    assert!(!samples[15].correlation.is_material_drop());
    assert!(samples[16].correlation.is_material_drop(), "it continues");
    assert_eq!(samples[16].correlation.episode.unwrap().samples, 4);
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
        unusual: vec![CorrelationCause::DiskIo],
        stall_seconds: Some(2.0),
        slow: true,
        ..CorrelationInsight::default()
    };
    collector.mark_unusual(&sample);
    assert!(tones(&collector.generation_history)[5]);
    assert!(tones(&collector.disk_history)[5]);
    assert!(!tones(&collector.generation_history)[4]);

    // Colour means impact: outside a slowdown an unusually slow sample and
    // an unusual reading are said in Diagnostics, and nothing is painted.
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
    for history in [
        &collector.generation_history,
        &collector.gpu_history,
        &collector.swap_history,
    ] {
        assert!(tones(history).iter().all(|unusual| !unusual));
    }

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

/// A 73-token reply at 130 tok/s takes about half a second: its only poll
/// divides its tokens over an interval it wrote for part of. On one server
/// that read 76 tok/s against a usual 131 and was marked unusual alone.
#[test]
fn a_request_that_starts_or_ends_between_polls_is_shown_not_judged() {
    let mut engine = CorrelationEngine::default();
    let request = |id: &str, second: u64, tps: f64| {
        let mut sample = paging(second, tps, 0);
        for running in &mut sample.llm_requests {
            running.id = id.into();
        }
        sample
    };
    let mut second = 0;
    // Short replies at about 130 tok/s, four polls each, teach the usual.
    for reply in 0..6 {
        for _ in 0..4 {
            let sample = request(&format!("r{reply}"), second, 130.0);
            engine.observe(&sample, defaults());
            second += 1;
        }
    }
    // The reply whose only poll is a partial interval.
    let short = engine.observe(&request("short", second, 76.0), defaults());
    assert!(!short.slow, "a partial interval is never slow");
    assert!(!short.is_material_drop());
    // The same speed over whole intervals of one request is unusual.
    let mut judged = Vec::new();
    for _ in 0..3 {
        second += 1;
        judged.push(engine.observe(&request("long", second, 76.0), defaults()));
    }
    assert!(!judged[0].slow, "its first poll is partial too");
    assert!(
        judged[1].slow && judged[2].slow,
        "whole intervals are judged"
    );
}

/// A prompt being read is the model's performance too: a slow prefill is
/// found by the same rule as a slow answer, against prefill's own usual
/// speed, and drawn on the prefill chart.
#[test]
fn a_slow_prefill_is_found_named_and_drawn_like_a_slow_answer() {
    let reading = |second: u64, tps: f64, swap: u64| {
        let mut sample = paging(second, 0.0, swap);
        sample.llm_status = "prefilling".into();
        sample.llm_generation_tps = None;
        sample.llm_generation_tps_live = false;
        sample.llm_prefill_tps = Some(tps);
        sample.llm_prefill_tps_live = true;
        for request in &mut sample.llm_requests {
            request.prompt = 30_000;
            request.cached = Some(0);
            request.output = None;
            request.output_tps = None;
            request.idle_for = None;
            request.prefilled = Some(second * 180);
        }
        sample
    };
    let swap = defaults().swap_warn_rate * 2;
    let mut samples: Vec<Sample> = (0..12).map(|second| reading(second, 180.0, 0)).collect();
    samples.extend((12..18).map(|second| reading(second, 70.0, swap)));
    let mut engine = CorrelationEngine::default();
    observe_all(&mut engine, &mut samples);

    let usual = &samples[11].correlation;
    assert_eq!(usual.phase, domain::Phase::Prefill);
    assert!(!usual.is_material_drop());
    assert_eq!(usual.usual_speed.map(|(speed, _)| speed), Some(180.0));
    assert!(
        !samples[13].correlation.is_material_drop(),
        "two slow polls"
    );
    let slow = &samples[14].correlation;
    assert!(slow.is_material_drop(), "three slow polls of prefill");
    assert_eq!(slow.phase, domain::Phase::Prefill);
    assert!(
        slow.summary
            .starts_with("PREFILL ↓61.1% (180.0→70.0 tok/s)"),
        "{}",
        slow.summary
    );
    assert!(
        slow.unusual.contains(&CorrelationCause::Paging),
        "{:?}",
        slow.summary
    );
    assert!(slow.concerns(ChartMetric::Prefill) && !slow.concerns(ChartMetric::Generation));
    // Generation has learned nothing from prompts being read.
    assert_eq!(engine.speed.level(), None);

    // It is drawn on the prefill chart, with paging beside it.
    let mut collector = Collector::with_host(
        60,
        offline_config(),
        Box::new(FakeHost::default()),
        Platform::MacOs,
        None,
    );
    for _ in 0..4 {
        for history in [
            &mut collector.prefill_history,
            &mut collector.generation_history,
            &mut collector.swap_history,
        ] {
            push_history_with_tone(history, Some(700), Tone::Green, 60);
        }
    }
    collector.mark_unusual(&samples[14]);
    let marked = |history: &VecDeque<ChartPoint>| {
        history
            .iter()
            .filter(|point| point.tone == Tone::Unusual)
            .count()
    };
    assert_eq!(marked(&collector.prefill_history), 3);
    assert_eq!(marked(&collector.swap_history), 3);
    assert_eq!(marked(&collector.generation_history), 0);
    assert!(crate::drop_marks::tag(slow, 40).contains("slowed PREFILL"));

    // An answer being written beside a prompt keeps the finding on generation.
    let mut both = beside(paging(20, 50.0, 0), 30_000, true);
    both.llm_prefill_tps = Some(70.0);
    both.llm_prefill_tps_live = true;
    assert_eq!(
        engine.observe(&both, defaults()).phase,
        domain::Phase::Generation
    );
}

/// Speculative decoding earning less is a slowdown on a healthy machine: it
/// is named as the reason, with the machine said not to be the limit.
#[test]
fn a_slowdown_from_fewer_draft_tokens_accepted_says_so() {
    let with_drafts = |second: u64, tps: f64, acceptance: f64| {
        let mut sample = paging(second, tps, 0);
        sample.llm_draft_acceptance = Some(acceptance);
        sample
    };
    let mut samples: Vec<Sample> = (0..12).map(|s| with_drafts(s, 60.0, 0.8)).collect();
    samples.extend((12..16).map(|s| with_drafts(s, 30.0, 0.35)));
    observe_all(&mut CorrelationEngine::default(), &mut samples);
    let slow = &samples[15].correlation;
    assert!(slow.is_material_drop());
    assert_eq!(
        slow.cause,
        CorrelationCause::DraftAcceptance,
        "{}",
        slow.summary
    );
    assert!(
        slow.lead.contains("draft acceptance 35% (usual 80%)"),
        "{}",
        slow.lead
    );
    let mut sample = samples[15].clone();
    sample.llm_source = TelemetrySource::Live;
    assert!(crate::diagnosis::slowdown_next_step(&sample).contains("draft model"));
    assert_eq!(
        crate::drop_marks::chart_of(CorrelationCause::DraftAcceptance),
        None
    );

    // The generation chart's edge says what it is earning.
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    app.collector.current.llm_draft_acceptance = Some(0.72);
    let history = app.collector.generation_history.clone();
    let screen = render_view(70, 8, |frame| {
        app.render_indicator_chart(
            frame,
            frame.area(),
            "generation",
            &history,
            ChartMetric::Generation,
        )
    });
    assert!(screen.contains("draft 72%"), "{screen}");
}
