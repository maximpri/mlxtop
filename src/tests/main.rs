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
            session: Default::default(),
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
        summary_open: false,
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
        session: Default::default(),
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

fn is_llm_process(name: &str, command: &str) -> bool {
    process_provider(name, command).is_some()
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

// --- runtime disk and SSD cache ------------------------------------------------

fn with_disk(mut sample: Sample, read: u64, loads: u64) -> Sample {
    sample.disk = domain::DiskActivity {
        read: Some(read),
        write: Some(0),
        loads,
        ..Default::default()
    };
    sample
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

// --- slowdowns: generation unusually slow, and what else is unusual -----

/// Ten polls at the usual 50 tok/s, then `rest`, observed in order.
fn after_usual(rest: Vec<Sample>) -> Vec<Sample> {
    let mut samples: Vec<Sample> = (0..10).map(|second| paging(second, 50.0, 0)).collect();
    samples.extend(rest);
    observe_all(&mut CorrelationEngine::default(), &mut samples);
    samples
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

// The tests, by topic. Each module sees the helpers above.
#[path = "main_config.rs"]
mod config;
#[path = "main_dashboards.rs"]
mod dashboards;
#[path = "main_detection.rs"]
mod detection;
#[path = "main_journal.rs"]
mod journal;
#[path = "main_other.rs"]
mod other;
#[path = "main_runtimes.rs"]
mod runtimes;
