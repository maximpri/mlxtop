// SPDX-License-Identifier: MIT
//! The QA judge on what context left adds: the forecast under what memory
//! has left, its arithmetic in Diagnostics, and the wired-limit advice.
use super::qa::{judge, Case, SIZES};
use super::*;
use crate::chart_navigation::Chart;
use crate::model_facts::ModelFacts;

const GIB: u64 = 1024 * MIB;
/// Int8 KV per token, and what Splash's plan leaves for requests, on the
/// owner's Mac Studio; and what a Splash request in flight holds.
const KV: u64 = 33_280;
const REMAINDER: u64 = 5_017_714_904;
const ACTIVE_STATE: u64 = 356_122_624;

fn owner_facts() -> ModelFacts {
    ModelFacts {
        bytes_per_pass: Some(15_020_479_488),
        kv_layers: Some(16),
        kv_heads: Some(4),
        head_dim: Some(256),
        kv_dtype: Some("int8".into()),
        kv_bytes_per_element: Some(1.0 + 1.0 / 64.0),
        fixed_state_bytes: Some(160_038_912),
        context_configured: Some(131_072),
        context_trained: Some(262_144),
        draft: Some(true),
        draft_bytes: Some(3_848_808_960),
        runtime_version: Some("oMLX 0.7.3 · Splash 1.5.0".into()),
        ..ModelFacts::default()
    }
}

fn running(contexts: &[u64]) -> Vec<domain::RequestUsage> {
    contexts
        .iter()
        .enumerate()
        .map(|(index, context)| domain::RequestUsage {
            provider: "oMLX".into(),
            model: "Qwen3.8-27B-oQ4e-mtp".into(),
            id: format!("live-{index}"),
            prompt: context - 400,
            output: Some(400),
            output_tps: Some(60.0),
            observed_at: Some(SystemTime::now()),
            ..domain::RequestUsage::default()
        })
        .collect()
}

/// The owner's Mac Studio: oMLX serving through Splash, with requests in
/// flight holding `contexts` and what Splash's plan has left after them.
fn splash(contexts: &[u64]) -> App {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    let sample = &mut app.collector.current;
    sample.llm_engine = Some("Splash".into());
    sample.model = owner_facts();
    sample.llm_requests = running(contexts);
    sample.llm_active_requests = Some(contexts.len() as u64);
    sample.llm_kv_free_bytes = Some(
        REMAINDER
            - contexts
                .iter()
                .map(|context| context * KV + ACTIVE_STATE)
                .sum::<u64>(),
    );
    app
}

/// oMLX's own engine, growing its KV cache in the Mac's memory; `gpu` of
/// a 27 GiB working set held.
fn growing(gpu: u64) -> App {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    let sample = &mut app.collector.current;
    sample.model = owner_facts();
    sample.model.fixed_state_bytes = Some(153_944_064);
    sample.wired = gpu + GIB;
    sample.anonymous = 3 * GIB;
    sample.compressor = GIB / 2;
    sample.file_backed = 3 * GIB;
    sample.gpu_in_use = Some(gpu);
    app
}

/// llama.cpp, which sized its KV cache for 32k tokens when it loaded.
fn llama() -> App {
    let mut app = growing(10 * GIB);
    let sample = &mut app.collector.current;
    sample.llm_provider = "llama.cpp".into();
    sample.model.context_configured = Some(32_768);
    sample.llm_requests = running(&[12_100]);
    app
}

/// With the history of what memory held: a chart under the rows.
fn with_history(mut app: App) -> App {
    let sample = app.collector.current.clone();
    let sizes = crate::memory_composition::segments(&sample).unwrap();
    app.collector.composition_history = (0..120).map(|_| Some(sizes)).collect();
    app.collector.gpu_memory_history = (0..120).map(|_| sample.gpu_in_use).collect();
    app
}

fn cases() -> Vec<(Case, Option<&'static str>)> {
    let case = |name, app| Case {
        name,
        impact: false,
        app,
    };
    let mut diagnostics = growing(25 * GIB);
    diagnostics.diagnostics_open = true;
    let mut unknown = splash(&[63_673]);
    unknown.collector.current.model.kv_bytes_per_element = None;
    vec![
        (
            case("context-left-splash", splash(&[63_673])),
            Some("≈67k tokens at 1 request"),
        ),
        (
            case("context-left-two-long", splash(&[63_673, 63_673])),
            Some("≈1k tokens at 2 requests"),
        ),
        (
            case("context-left-median", splash(&[1_932])),
            Some("≈129k tokens at 1 request"),
        ),
        (
            case("context-left-idle", splash(&[])),
            Some("≈131k tokens at 1 request"),
        ),
        (
            case("context-left-growing", growing(18 * GIB)),
            Some("≈97k tokens at 1 request"),
        ),
        (
            case("context-left-near-limit", growing(25 * GIB)),
            Some("≈32k tokens at 1 request"),
        ),
        (
            case("context-left-llama", llama()),
            Some("≈20k tokens at 1 request"),
        ),
        (
            case("context-left-history", with_history(splash(&[63_673]))),
            Some("≈67k tokens at 1 request"),
        ),
        (case("context-left-unknown", unknown), None),
        (case("context-left-diagnostics", diagnostics), None),
    ]
}

/// The memory panel's rows, as text, after drawing `app`.
fn memory_rows(app: &App, width: u16, height: u16) -> Vec<String> {
    let screen = render_app(app, width, height);
    let rows: Vec<Vec<char>> = screen.lines().map(|row| row.chars().collect()).collect();
    let region = app
        .charts
        .regions
        .borrow()
        .iter()
        .find(|(chart, _)| *chart == Chart::Memory)
        .map(|(_, region)| *region);
    let Some(region) = region else {
        return Vec::new();
    };
    (region.y + 1..region.bottom().saturating_sub(1))
        .map(|y| {
            rows[usize::from(y)][usize::from(region.x + 1)..usize::from(region.right() - 1)]
                .iter()
                .collect::<String>()
        })
        .collect()
}

#[test]
fn context_left_passes_the_qa_judge_at_every_size() {
    let mut failures = Vec::new();
    for (case, label) in cases() {
        for (width, height) in SIZES {
            for problem in judge(&case, width, height) {
                failures.push(format!("{} at {width}×{height}: {problem}", case.name));
            }
            let screen = render_app(&case.app, width, height);
            let shown = screen.matches("tokens at ").count();
            // Whole or not at all, and once. At 80×24 only where a row is
            // free (the_forecast_takes_no_row_from_anything_else); the
            // summary's popup leaves the panel in view on large screens.
            let expected: &[usize] = if case.app.summary_open {
                &[0, 1]
            } else if label.is_none() || case.app.diagnostics_open {
                &[0]
            } else if width == 80 {
                &[0, 1]
            } else {
                &[1]
            };
            if !expected.contains(&shown)
                || label.is_some_and(|label| shown > 0 && !screen.contains(label))
            {
                failures.push(format!(
                    "{} at {width}×{height}: forecast shown {shown} times, expected {expected:?} ({label:?})\n{screen}",
                    case.name
                ));
            }
        }
    }
    assert!(failures.is_empty(), "QA judge:\n{}", failures.join("\n"));
}

#[test]
fn the_forecast_takes_no_row_from_anything_else() {
    // The history chart starts at its top label: all of RAM.
    let chart = |rows: &[String]| rows.iter().position(|row| row.starts_with("36.0 GiB"));
    for (case, label) in cases() {
        let Some(label) = label else {
            continue;
        };
        // The same screen with the window unknown: no forecast.
        let mut blind = test_app(0);
        blind.collector = case.app.collector.clone();
        blind.collector.current.model.context_configured = None;
        blind.collector.current.model.context_trained = None;
        for (width, height) in SIZES {
            let mut with = memory_rows(&case.app, width, height);
            let without = memory_rows(&blind, width, height);
            let at = with.iter().position(|row| row.contains(label));
            if let Some(at) = at {
                // Right under what is left, and right-aligned like it.
                assert!(
                    with[at].ends_with(label)
                        && (with[at - 1].ends_with(" left") || with[at - 1].ends_with("GPU limit")),
                    "{} at {width}×{height}: {with:#?}",
                    case.name
                );
                with.remove(at);
            }
            // Every row above the history is as it was, in its place; the
            // history may give up a row, never go.
            let (top, before) = (chart(&with), chart(&without));
            assert_eq!(
                top.is_some(),
                before.is_some(),
                "{} at {width}×{height}: the history went\n{with:#?}",
                case.name
            );
            let (top, before) = (top.unwrap_or(with.len()), before.unwrap_or(without.len()));
            let blank = |rows: &[String]| rows.iter().all(|row| row.trim().is_empty());
            let (kept, was) = if top == with.len() && before == without.len() {
                // No history: the forecast took a blank row at the bottom.
                assert!(
                    at.is_none() || blank(&without[with.len()..]),
                    "{}",
                    case.name
                );
                (&with[..], &without[..with.len()])
            } else {
                (&with[..top], &without[..before])
            };
            assert_eq!(kept, was, "{} at {width}×{height}", case.name);
        }
    }
}

#[test]
fn at_80_by_24_the_forecast_goes_first() {
    // The owner's Overview at 80×24: the panel's rows go to RAM, what is
    // left, the GPU note and the legend; the forecast is the one dropped.
    for app in [splash(&[63_673]), splash(&[63_673, 63_673]), splash(&[])] {
        let rows = memory_rows(&app, 80, 24);
        assert_eq!(
            rows.iter().map(|row| row.trim()).collect::<Vec<_>>(),
            [
                "RAM       17.2 / 36.0 GiB",
                "9.0 GiB to GPU limit",
                "GPU 67% of 27.0 GiB",
                "█wired █app █comp █cache",
            ]
        );
    }
}

#[test]
fn the_forecast_colours_nothing() {
    let app = splash(&[63_673, 63_673]);
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(160, 48)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    let buffer = terminal.backend().buffer();
    let row = (0..48)
        .find(|y| {
            (0..160)
                .map(|x| buffer[(x, *y)].symbol())
                .collect::<String>()
                .contains("≈1k tokens at 2 requests")
        })
        .expect("forecast drawn");
    let line: String = (0..160).map(|x| buffer[(x, row)].symbol()).collect();
    let start = line.find('≈').unwrap();
    let start = line[..start].chars().count() as u16;
    for x in start..start + 24 {
        assert_eq!(
            buffer[(x, row)].fg,
            crate::theme::MUTED,
            "plain text, not a grade"
        );
    }
}

#[test]
fn diagnostics_give_the_arithmetic_after_the_models_facts() {
    let mut app = splash(&[63_673, 63_673]);
    app.diagnostics_open = true;
    let lines = crate::report::diagnostic_lines(&app.collector.current);
    let facts = lines
        .iter()
        .position(|line| line.starts_with("Context: KV 32.5 KiB per token"))
        .expect("the model's facts");
    assert_eq!(
        lines[facts + 2..facts + 6],
        [
            "Context left: ≈1k more tokens for each of the 2 requests in flight (estimate)",
            "By memory: 64.3 MiB left in Splash's memory plan ÷ (2 requests × 32.5 KiB a token) = 1.0k tokens each, the smaller",
            "By window: 131.1k tokens allowed − 63.7k held = 67.4k tokens each",
            "Held: 127.3k tokens in 2 requests: 3.9 GiB of KV cache and ≈610.5 MiB of state (305.2 MiB each), 98% of what Splash's plan has for requests (4.6 GiB)",
        ]
    );
    // Near the GPU limit, the advice is in the assessment, in words only.
    let near = growing(25 * GIB);
    let lines = crate::report::diagnostic_lines(&near.collector.current);
    let advice = "GPU memory is 25.0 GiB of Metal's 27.0 GiB recommended working set, where MLX warns that generation can be slow: raise iogpu.wired_limit_mb (sudo sysctl iogpu.wired_limit_mb=…) or shorten the context.";
    let at = lines
        .iter()
        .position(|line| line == advice)
        .expect("advice");
    assert!(at < lines.iter().position(|line| line == "CONNECTION").unwrap());
    assert_eq!(
        crate::diagnosis::assess(&near.collector.current).title,
        "Healthy · no bottleneck",
        "the verdict is unchanged"
    );
    // Unknown inputs: no forecast, no arithmetic.
    let mut unknown = splash(&[63_673]);
    unknown.collector.current.model.kv_bytes_per_element = None;
    assert!(!crate::report::diagnostic_lines(&unknown.collector.current)
        .iter()
        .any(|line| line.starts_with("Context left")));
}

/// The memory panel alone, `width` × `height`, for a machine whose GPU
/// memory is `memory`, with a history of the sample's own composition.
fn panel(sample: &Sample, memory: gpu::Memory, width: u16, height: u16) -> Vec<String> {
    let sizes = crate::memory_composition::segments(sample).unwrap();
    let gpu = match memory {
        gpu::Memory::Dedicated { used, .. } => Some(used),
        _ => sample.gpu_in_use,
    };
    let screen = render_view(width, height, |frame| {
        crate::memory_headroom::draw_for(frame, frame.area(), sample, memory, |columns| {
            vec![
                Some(crate::memory_headroom::Column {
                    sizes,
                    gpu,
                    tone: Tone::Green,
                });
                columns
            ]
        });
    });
    screen.lines().map(str::to_owned).collect()
}

#[test]
fn the_forecast_yields_at_every_panel_height() {
    let dedicated = gpu::Memory::Dedicated {
        used: 20 * GIB,
        total: 24 * GIB,
    };
    let mut seen = [false; 2];
    for memory in [gpu::Memory::Shared, dedicated] {
        let app = splash(&[63_673]);
        let sample = app.collector.current.clone();
        let mut blind = sample.clone();
        blind.model.context_configured = None;
        blind.model.context_trained = None;
        // Where the history starts: its top label, VRAM's or RAM's.
        let chart = |rows: &[String]| {
            rows.iter()
                .position(|row| row.starts_with("24.0 GiB") || row.starts_with("36.0 GiB"))
        };
        let charts = |rows: &[String]| {
            rows.iter()
                .filter(|row| row.starts_with("24.0 GiB") || row.starts_with("36.0 GiB"))
                .count()
        };
        for width in [26, 40, 66] {
            for height in 1..=14 {
                let mut with = panel(&sample, memory, width, height);
                let without = panel(&blind, memory, width, height);
                if let Some(at) = with
                    .iter()
                    .position(|row| row.ends_with("tokens at 1 request"))
                {
                    seen[usize::from(memory != gpu::Memory::Shared)] = true;
                    with.remove(at);
                    with.push(" ".repeat(usize::from(width)));
                }
                let context =
                    format!("{memory:?} {width}×{height}\nwith: {with:#?}\nwithout: {without:#?}");
                // Never at the cost of a history chart, and every row above
                // the history as it was, in its place. Without a history the
                // forecast took a blank row at the bottom.
                assert_eq!(charts(&with), charts(&without), "{context}");
                let top = chart(&with);
                assert_eq!(top, chart(&without), "{context}");
                let end = top.unwrap_or(with.len());
                assert_eq!(with[..end], without[..end], "{context}");
            }
        }
    }
    assert_eq!(
        seen,
        [true, true],
        "the forecast found room in both layouts"
    );
}

#[test]
fn small_forecasts_and_stateless_models_read_plainly() {
    // One token left, and a model that holds no state for a request.
    let mut app = splash(&[131_071]);
    assert_eq!(
        crate::context_left::forecast_for(&app.collector.current, gpu::Memory::Shared)
            .map(|forecast| forecast.label())
            .as_deref(),
        Some("≈1 token at 1 request"),
        "the window's last token"
    );
    let sample = &mut app.collector.current;
    sample.model.fixed_state_bytes = Some(0);
    sample.llm_kv_free_bytes = Some(KV * 4);
    let lines = crate::context_left::lines(sample);
    assert_eq!(
        lines,
        [
            "Context left: ≈1 more token for the request in flight (estimate)",
            "By memory: 130.0 KiB left in Splash's memory plan ÷ 32.5 KiB a token = 4 tokens",
            "By window: 131.1k tokens allowed − 131.1k held = 1 token, the smaller",
            "Held: 131.1k tokens in 1 request: 4.1 GiB of KV cache, 99% of what Splash's plan has for requests (4.1 GiB)",
        ]
    );
}
