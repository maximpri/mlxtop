// SPDX-License-Identifier: MIT
//! The QA judge: every screen mlxtop draws, in every state that matters and at
//! every common terminal size, checked against the rules its design rests on.
//! A release does not ship while one fails. `scripts/qa_judge.py` adds a
//! model's review of the same screens (see `qa_screens_for_the_model_judge`).
use super::*;
use crate::theme::UNUSUAL;
use ratatui::buffer::Buffer;

/// Terminal sizes people run mlxtop in: a default window up to a big one.
const SIZES: [(u16, u16); 5] = [(80, 24), (100, 30), (120, 40), (160, 48), (200, 60)];

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
/// may show the anomaly colour), and the app in it.
struct Case {
    name: &'static str,
    impact: bool,
    app: App,
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
            name: "stall",
            impact: true,
            app: slowed(
                healthy(),
                CorrelationCause::Stall,
                "no new token for 3s · GPU idle",
            ),
        },
    ]
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
fn judge(case: &Case, width: u16, height: u16) -> Vec<String> {
    let buffer = draw(&case.app, width, height);
    let rows = text(&buffer);
    let screen = rows.join("\n");
    let mut problems = Vec::new();
    // Colour means impact: magenta only while the model is slowed or stalled.
    let magenta = buffer
        .content
        .iter()
        .filter(|cell| cell.fg == UNUSUAL || cell.bg == UNUSUAL)
        .count();
    if !case.impact && magenta > 0 {
        problems.push(format!(
            "{magenta} magenta cells with nothing slowing the model"
        ));
    }
    if case.impact && magenta == 0 && !case.app.summary_open && !case.app.diagnostics_open {
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
    let overlay = case.app.summary_open || case.app.diagnostics_open;
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
    // Magenta on a healthy screen.
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
    assert!(judge(&case, 120, 40).iter().any(|p| p.contains("magenta")));
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
    for case in cases() {
        for (width, height) in SIZES {
            let buffer = draw(&case.app, width, height);
            let rows = text(&buffer);
            let mut out = rows.join("\n");
            let magenta: Vec<String> = rows
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
                "\n\n[state: {} · the model is {}]\n[magenta cells: {}]\n",
                case.name,
                if case.impact { "slowed" } else { "not slowed" },
                if magenta.is_empty() {
                    "none".into()
                } else {
                    magenta.join("; ")
                }
            ));
            std::fs::write(dir.join(format!("{}-{width}x{height}.txt", case.name)), out).unwrap();
        }
    }
}
