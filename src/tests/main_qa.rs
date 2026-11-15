// SPDX-License-Identifier: MIT
//! The QA judge: every screen mlxtop draws, in every state that matters and at
//! every common terminal size, checked against the rules its design rests on.
//! A release does not ship while one fails. `scripts/qa_judge.py` adds a
//! model's review of the same screens (see `qa_screens_for_the_model_judge`).
use super::*;
use crate::theme::UNUSUAL;
use ratatui::buffer::Buffer;

/// Terminal sizes people run mlxtop in: a default window up to a big one.
pub(super) const SIZES: [(u16, u16); 5] = [(80, 24), (100, 30), (120, 40), (160, 48), (200, 60)];

/// Readouts and sentences the design removed as noise. None may come back.
const NOISE: [&str; 9] = [
    "% resident",
    "Light compression",
    "No disk traffic",
    "No paging traffic in this window",
    "swap off",
    "read 0 B/s",
    "page-in ",
    "no swap file yet",
    "cache I/O",
];

/// What a broken value looks like on screen.
const BROKEN: [&str; 6] = [
    "NaN",
    "inf tok",
    "-0.0",
    "18446744073709551615",
    "None",
    "Some(",
];

/// A state the judge renders: its name, whether the model is slowed (and so
/// may show the anomaly colour), and the app in it. Packages judge their
/// own states from their own test files.
pub(super) struct Case {
    pub(super) name: &'static str,
    pub(super) impact: bool,
    pub(super) app: App,
}

fn slowed(mut app: App, cause: CorrelationCause, lead: &str) -> App {
    let insight = CorrelationInsight {
        direction: ThroughputDirection::Down,
        cause,
        lead: lead.into(),
        summary: format!("GEN ↓50.0% (50.0→25.0 tok/s) · {lead}"),
        delta_percent: Some(-50.0),
        slow: true,
        unusual: if cause == CorrelationCause::Stall {
            Vec::new()
        } else {
            vec![cause]
        },
        notes: vec![lead.into()],
        episode: Some(domain::DropEpisode {
            reference: 50.0,
            low: 25.0,
            seconds: 6.0,
            samples: 6,
        }),
        usual_speed: Some((50.0, 40.0)),
        stall_seconds: (cause == CorrelationCause::Stall).then_some(3.0),
        ..CorrelationInsight::default()
    };
    app.collector.current.correlation = insight;
    // The slowdown's own samples, and the signal it is given as a reason
    // for; a stall with an idle machine marks only generation.
    let mut marked = vec![&mut app.collector.generation_history];
    if cause == CorrelationCause::Paging {
        marked.push(&mut app.collector.swap_history);
    }
    for history in marked {
        for point in history.iter_mut().rev().take(3) {
            if point.value.is_some() {
                point.tone = Tone::Unusual;
            }
        }
    }
    app
}

fn cases() -> Vec<Case> {
    let healthy = || {
        let mut app = test_app(0);
        populate_dashboard_fixture(&mut app);
        app
    };
    let mut prefill = healthy();
    prefill.collector.current.llm_status = "prefilling".into();
    prefill.collector.current.llm_prefill_tokens_done = Some(12_000);
    prefill.collector.current.llm_prefill_tokens_expected = Some(37_000);
    let mut linux = healthy();
    linux.collector.platform = Platform::Linux;
    let mut summary = healthy();
    summary.summary_open = true;
    let mut diagnostics = healthy();
    diagnostics.diagnostics_open = true;
    // Every other view, the empty ones, Help, a discrete-GPU host, Windows.
    let on_tab = |tab: usize| {
        let mut app = healthy();
        app.tab = tab;
        app
    };
    let mut help = healthy();
    help.help = true;
    let mut windows = healthy();
    windows.collector.platform = Platform::Windows;
    let more = vec![
        Case {
            name: "processes",
            impact: false,
            app: on_tab(1),
        },
        Case {
            name: "journal",
            impact: false,
            app: on_tab(2),
        },
        Case {
            name: "settings",
            impact: false,
            app: on_tab(3),
        },
        Case {
            name: "processes-empty",
            impact: false,
            app: test_app(1),
        },
        Case {
            name: "journal-empty",
            impact: false,
            app: test_app(2),
        },
        Case {
            name: "help",
            impact: false,
            app: help,
        },
        Case {
            name: "nvidia",
            impact: false,
            app: nvidia_app(2),
        },
        Case {
            name: "windows",
            impact: false,
            app: windows,
        },
    ];
    // A slowdown while a host state is active: the slowdown leads.
    let mut paging_slowdown = slowed(
        healthy(),
        CorrelationCause::Paging,
        "paging 6.0 MiB/s (usual 0.5 MiB/s)",
    );
    paging_slowdown.collector.current.impact = "PAGING ACTIVE".into();
    paging_slowdown.collector.current.impact_tone = Tone::Yellow;
    // macOS holding the Mac back for heat: news on the card, and yellow
    // only beside a slowdown it explains.
    let hot = || {
        let mut app = healthy();
        let current = &mut app.collector.current;
        current.thermal_state.level = Some(ThermalLevel::Heavy);
        current.thermal = current.thermal_state.label();
        current.system_power_mw = Some(48_000);
        current.impact = "THERMAL LIMIT".into();
        current.impact_tone = Tone::Yellow;
        app
    };
    vec![
        Case {
            name: "idle",
            impact: false,
            app: test_app(0),
        },
        Case {
            name: "healthy",
            impact: false,
            app: healthy(),
        },
        Case {
            name: "prefilling",
            impact: false,
            app: prefill,
        },
        Case {
            name: "linux",
            impact: false,
            app: linux,
        },
        Case {
            name: "session-summary",
            impact: false,
            app: summary,
        },
        Case {
            name: "diagnostics",
            impact: false,
            app: diagnostics,
        },
        Case {
            name: "slowdown-paging",
            impact: true,
            app: slowed(
                healthy(),
                CorrelationCause::Paging,
                "paging 96.0 MiB/s (usual 2.0 MiB/s)",
            ),
        },
        Case {
            name: "slowdown-with-paging-active",
            impact: true,
            app: paging_slowdown,
        },
        Case {
            name: "stall",
            impact: true,
            app: slowed(
                healthy(),
                CorrelationCause::Stall,
                "no new token for 3s · GPU idle",
            ),
        },
        Case {
            name: "thermal-heavy",
            impact: false,
            app: hot(),
        },
        Case {
            name: "slowdown-thermal",
            impact: true,
            app: slowed(hot(), CorrelationCause::Thermal, "thermal pressure heavy"),
        },
    ]
    .into_iter()
    .chain(more)
    .collect()
}

fn draw(app: &App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    app.charts.regions.borrow_mut().clear();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    terminal.backend().buffer().clone()
}

fn text(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    (0..area.height)
        .map(|y| (0..area.width).map(|x| buffer[(x, y)].symbol()).collect())
        .collect()
}

/// Every rule a screen breaks, in words.
pub(super) fn judge(case: &Case, width: u16, height: u16) -> Vec<String> {
    judge_screen(case, &draw(&case.app, width, height), width, height)
}

/// The rules, over a screen already drawn.
fn judge_screen(case: &Case, buffer: &Buffer, width: u16, height: u16) -> Vec<String> {
    let rows = text(buffer);
    let screen = rows.join("\n");
    let mut problems = Vec::new();
    // Colour means impact: yellow only while the model is slowed or stalled.
    let yellow = buffer
        .content
        .iter()
        .filter(|cell| cell.fg == UNUSUAL || cell.bg == UNUSUAL)
        .count();
    if !case.impact && yellow > 0 {
        problems.push(format!(
            "{yellow} yellow cells with nothing slowing the model"
        ));
    }
    if case.impact && yellow == 0 && !case.app.summary_open && !case.app.diagnostics_open {
        problems.push("a slowdown that no chart shows".into());
    }
    for noise in NOISE {
        if screen.contains(noise) {
            problems.push(format!("noise came back: {noise:?}"));
        }
    }
    for broken in BROKEN {
        if screen.contains(broken) {
            problems.push(format!("broken value on screen: {broken:?}"));
        }
    }
    // Every panel the Overview promises is there, with its border whole.
    let overlay = case.app.summary_open || case.app.diagnostics_open || case.app.help;
    if case.app.tab == 0 && !overlay && width >= 100 {
        let mut titles = vec![
            "memory",
            "KV cache",
            "paging",
            "prompt load",
            "generation",
            "prefill",
            "GPU",
            "queue",
        ];
        if case.app.collector.platform == Platform::MacOs {
            titles.push("compression");
        }
        for title in titles {
            if !screen.contains(title) {
                problems.push(format!("panel missing: {title}"));
            }
        }
        for (chart, region) in case.app.charts.regions.borrow().iter() {
            if region.width < 4 || region.height < 2 {
                continue;
            }
            let corners = [
                (region.x, region.y, "┌╔╭┏"),
                (region.right() - 1, region.bottom() - 1, "┘╝╯┛"),
            ];
            for (x, y, glyphs) in corners {
                if x < width && y < height && !glyphs.contains(buffer[(x, y)].symbol()) {
                    problems.push(format!(
                        "{chart:?} border broken at ({x}, {y}): {:?}",
                        buffer[(x, y)].symbol()
                    ));
                }
            }
        }
    }
    // The key bar fits: quitting is always one readable key away.
    if width >= 120 && !rows.last().is_some_and(|row| row.contains("q quit")) {
        problems.push(format!("key bar lost `q quit`: {:?}", rows.last()));
    }
    problems
}

#[test]
fn every_screen_passes_the_qa_judge() {
    let mut failures = Vec::new();
    for case in cases() {
        for (width, height) in SIZES {
            for problem in judge(&case, width, height) {
                failures.push(format!("{} at {width}×{height}: {problem}", case.name));
            }
        }
    }
    assert!(failures.is_empty(), "QA judge:\n{}", failures.join("\n"));
}

#[test]
fn the_qa_judge_catches_what_it_is_for() {
    // Yellow on a healthy screen.
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    for point in app.collector.swap_history.iter_mut() {
        point.tone = Tone::Unusual;
    }
    let case = Case {
        name: "planted",
        impact: false,
        app,
    };
    assert!(judge(&case, 120, 40).iter().any(|p| p.contains("yellow")));
    // A slowdown no chart shows.
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    let case = Case {
        name: "unseen",
        impact: true,
        app,
    };
    assert!(judge(&case, 120, 40)
        .iter()
        .any(|p| p.contains("no chart shows")));
}

/// Writes every screen as text for `scripts/qa_judge.py`, with the cells in
/// the anomaly colour listed under it, since a model reads no colour.
#[test]
#[ignore = "run by scripts/qa_judge.py"]
fn qa_screens_for_the_model_judge() {
    let Some(dir) = std::env::var_os("QA_SCREENS_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    type Drawer = fn(&App, u16, u16) -> Buffer;
    let screens = cases()
        .into_iter()
        .map(|case| (case, draw as Drawer))
        .chain(
            device_cases()
                .into_iter()
                .map(|case| (case, draw_devices as Drawer)),
        );
    for (case, drawer) in screens {
        for (width, height) in SIZES {
            let buffer = drawer(&case.app, width, height);
            let rows = text(&buffer);
            let mut out = rows.join("\n");
            let yellow: Vec<String> = rows
                .iter()
                .enumerate()
                .filter_map(|(y, row)| {
                    let cells: String = row
                        .chars()
                        .enumerate()
                        .filter(|(x, _)| {
                            let cell = &buffer[(*x as u16, y as u16)];
                            cell.fg == UNUSUAL || cell.bg == UNUSUAL
                        })
                        .map(|(_, c)| c)
                        .collect();
                    (!cells.trim().is_empty()).then(|| format!("row {y}: {cells}"))
                })
                .collect();
            out.push_str(&format!(
                "\n\n[state: {} · the model is {}]\n[yellow cells: {}]\n",
                case.name,
                if case.impact { "slowed" } else { "not slowed" },
                if yellow.is_empty() {
                    "none".into()
                } else {
                    yellow.join("; ")
                }
            ));
            std::fs::write(dir.join(format!("{}-{width}x{height}.txt", case.name)), out).unwrap();
        }
    }
}

/// The Overview as Linux and Windows draw it for NVIDIA and AMD cards, on
/// any host: the per-card table, then the same panels.
fn draw_devices(app: &App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    app.charts.regions.borrow_mut().clear();
    terminal
        .draw(|frame| {
            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(5),
                Constraint::Length(1),
            ])
            .split(frame.area());
            app.draw_controls(frame, rows[2]);
            app.draw_gpu_overview(frame, rows[1]);
            app.draw_header(frame, rows[0]);
            app.charts.decorate(frame);
            if app.diagnostics_open {
                app.draw_diagnostics(frame, rows[1]);
            }
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

/// Cards on a Linux host, with power, clocks, memory activity and links
/// read as `nvidia-smi` prints them.
fn linux_cards(count: usize) -> App {
    let mut app = nvidia_app(count);
    app.collector.platform = Platform::Linux;
    app.collector.current.thermal = "78°C measured".into();
    let rows: String = app
        .collector
        .current
        .gpus
        .iter()
        .enumerate()
        .map(|(i, gpu)| {
            format!(
                "{}, {}, 450.00, {}, 3105, {}, 4, 4, 16, {}\n",
                gpu.uuid,
                [448.2, 301.7, 31.0, 402.9][i % 4],
                [2010, 2610, 210, 2475][i % 4],
                [93, 64, 0, 88][i % 4],
                [16, 16, 16, 8][i % 4]
            )
        })
        .collect();
    gpu::apply_limits(&mut app.collector.current.gpus, &rows, Instant::now());
    // The first card is held at its power cap.
    app.collector.current.gpus[0].throttle_reasons = Some(0x4);
    app
}

/// A machine whose GPU has no memory of its own: RAM by kind, the GPU's
/// part of it, and `device` as its only card.
fn unified(device: gpu::Device) -> App {
    let mut app = linux_cards(1);
    let sample = &mut app.collector.current;
    sample.total_memory = 128 * 1024 * MIB;
    sample.wired = 66 * 1024 * MIB;
    sample.anonymous = 9 * 1024 * MIB;
    sample.file_backed = 31 * 1024 * MIB;
    sample.gpus = vec![device];
    sample.gpu_in_use = gpu::shared_in_use(&sample.gpus);
    sample.gpu_util = gpu::peak_utilization(&sample.gpus);
    classify(sample, None, defaults());
    app
}

fn device_cases() -> Vec<Case> {
    let mut spark = gpu::parse("0, GPU-5f3c, NVIDIA GB10, 96, [N/A], [N/A], 71\n");
    gpu::apply_limits(
        &mut spark,
        "GPU-5f3c, 37.12, [N/A], 2405, 3003, [N/A], [N/A], [N/A], [N/A], [N/A]\n",
        Instant::now(),
    );
    gpu::apply_processes(&mut spark, "GPU-5f3c, 4242, 62464\n");
    let halo = gpu::Device {
        index: 0,
        uuid: "AMD-0000:c5:00.0".into(),
        vendor: gpu::Vendor::Amd,
        name: "AMD Radeon Graphics / Radeon 8050S / 8060S Graphics".into(),
        utilization: Some(97),
        used: None,
        total: None,
        temperature: Some(64),
        throttle_reasons: None,
        unified: true,
        limits: gpu::Limits {
            power_mw: Some(86_000),
            clock_mhz: Some(2826),
            clock_max_mhz: Some(2900),
            ram_used: Some(58 * 1024 * MIB + 384 * MIB),
            ram_limit: Some(112 * 1024 * MIB + 512 * MIB),
            ..gpu::Limits::default()
        },
    };
    // Generation slowed while the first card held its clocks for heat: the
    // GPU panel is the reason's panel, marked as the collector marks it.
    let mut heat = linux_cards(2);
    heat.collector.current.gpus[0].throttle_reasons = Some(0x40);
    let mut heat = slowed(heat, CorrelationCause::Thermal, "thermal limit");
    for point in heat.collector.gpu_history.iter_mut().rev().take(3) {
        point.tone = Tone::Unusual;
    }
    let mut evidence = linux_cards(2);
    evidence.diagnostics_open = true;
    vec![
        Case {
            name: "nvidia-limits",
            impact: false,
            app: linux_cards(2),
        },
        Case {
            name: "nvidia-one",
            impact: false,
            app: linux_cards(1),
        },
        Case {
            name: "nvidia-eight",
            impact: false,
            app: linux_cards(8),
        },
        Case {
            name: "dgx-spark",
            impact: false,
            app: unified(spark.remove(0)),
        },
        Case {
            name: "strix-halo",
            impact: false,
            app: unified(halo),
        },
        Case {
            name: "nvidia-heat-slowdown",
            impact: true,
            app: heat,
        },
        Case {
            name: "nvidia-diagnostics",
            impact: false,
            app: evidence,
        },
    ]
}

#[test]
fn gpu_device_layouts_pass_the_qa_judge() {
    let mut failures = Vec::new();
    for case in device_cases() {
        for (width, height) in SIZES {
            let screen = draw_devices(&case.app, width, height);
            for problem in judge_screen(&case, &screen, width, height) {
                failures.push(format!("{} at {width}×{height}: {problem}", case.name));
            }
            // The per-card table keeps utilization, memory and temperature
            // whole at every width.
            let rows = text(&screen).join("\n");
            if !case.app.diagnostics_open {
                for label in ["GPU DEVICES", "UTILIZATION", "TEMP"] {
                    if !rows.contains(label) {
                        failures.push(format!("{} at {width}×{height}: lost {label}", case.name));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "QA judge:\n{}", failures.join("\n"));
}

#[test]
fn the_gpu_panel_edge_adds_power_and_memory_activity_while_it_has_room() {
    let app = linux_cards(1);
    let narrow = text(&draw_devices(&app, 80, 24)).join("\n");
    assert!(narrow.contains("saturated · 448/450 W"), "{narrow}");
    assert!(!narrow.contains("mem busy"), "{narrow}");
    let wide = text(&draw_devices(&app, 200, 60)).join("\n");
    assert!(
        wide.contains("saturated · 448/450 W · mem busy 93%"),
        "{wide}"
    );
    // Two cards: what they draw together; no single card's memory.
    let two = text(&draw_devices(&linux_cards(2), 200, 60)).join("\n");
    assert!(two.contains("saturated · 750/900 W ─"), "{two}");
}

// Model facts in Diagnostics, judged as every screen is.
#[path = "main_qa_model_facts.rs"]
mod model_facts;

// What Splash and oMLX say about the draft and what held work up, judged as every screen is.
#[path = "main_qa_runtime_attribution.rs"]
mod runtime_attribution;
