use crate::test_support::*;
// SPDX-License-Identifier: MIT
use super::*;

fn live() -> Sample {
    Sample {
        llm_source: TelemetrySource::Live,
        llm_status: "idle".into(),
        llm_provider: "oMLX".into(),
        llm_observed_at: Some(SystemTime::now()),
        llm_active_requests: Some(0),
        llm_waiting_requests: Some(0),
        ..Sample::default()
    }
}

#[test]
fn queue_keeps_idle_zero_but_gaps_stale_and_reported_values() {
    let mut history = History::default();
    let mut sample = live();
    history.observe(&sample, 10);
    assert_eq!(history.points.back().unwrap().active, Some(0));
    sample.llm_observed_at = Some(SystemTime::now() - Duration::from_secs(20));
    history.observe(&sample, 10);
    assert_eq!(history.points.back().unwrap().active, None);
    sample.llm_observed_at = Some(SystemTime::now());
    sample.llm_source = TelemetrySource::Report;
    history.observe(&sample, 10);
    assert_eq!(history.points.back().unwrap().waiting, None);
    sample.llm_source = TelemetrySource::Live;
    sample.llm_waiting_requests = None;
    history.observe(&sample, 2);
    assert_eq!(history.points.len(), 2);
    assert_eq!(history.points.back().unwrap().waiting, None);
}

#[test]
fn latency_requires_explicit_measurement_and_deduplicates_requests() {
    let mut sample = live();
    sample.llm_requests.push(domain::RequestUsage {
        provider: "oMLX".into(),
        model: "test".into(),
        id: "one".into(),
        prompt: 100,
        cached: None,
        output: None,
        completed: true,
        observed_at: Some(SystemTime::now()),
        queued: false,
        prefilled: None,
        ttft_ms: None,
        output_tps: None,
    });
    let mut history = History::default();
    history.observe(&sample, 10);
    assert!(!history.has_latency());
    sample.llm_requests[0].ttft_ms = Some(0);
    history.observe(&sample, 10);
    history.observe(&sample, 10);
    assert!(history.has_latency());
    assert_eq!(history.timings.len(), 1);
    sample.llm_requests[0].model = "other".into();
    history.observe(&sample, 10);
    assert_eq!(history.timings.len(), 2);
}

#[test]
fn stepped_trace_uses_connected_corners_for_rises_and_falls() {
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(3, 5)).unwrap();
    terminal
        .draw(|frame| {
            trace(
                frame,
                frame.area(),
                &[Some(0), Some(16), Some(0)],
                &[false; 3],
                16,
                CYAN,
                1,
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(1, 4)].symbol(), "┛");
    assert_eq!(buffer[(1, 0)].symbol(), "┏");
    assert_eq!(buffer[(2, 0)].symbol(), "┓");
    assert_eq!(buffer[(2, 4)].symbol(), "┗");
}

#[test]
fn zero_queue_series_share_labeled_baseline_and_keep_border_intact() {
    let mut history = History::default();
    for _ in 0..40 {
        history.observe(&live(), 80);
    }
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(44, 10)).unwrap();
    terminal
        .draw(|frame| {
            queue(
                frame,
                frame.area(),
                &history,
                Duration::from_secs(1),
                1,
                None,
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(40, 8)].symbol(), "═");
    assert_eq!(buffer[(40, 8)].fg, Color::White);
    assert_eq!(buffer[(1, 8)].symbol(), "0");
    assert!((2..8).all(|y| buffer[(40, y)].symbol() == " "));
    assert_eq!(buffer[(1, 9)].symbol(), "─");
}

#[test]
fn unequal_queue_counts_use_one_scale_and_missing_series_is_not_overlap() {
    let mut history = History::default();
    let mut sample = live();
    sample.llm_active_requests = Some(8);
    for _ in 0..40 {
        history.observe(&sample, 80);
    }
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(44, 10)).unwrap();
    terminal
        .draw(|frame| {
            queue(
                frame,
                frame.area(),
                &history,
                Duration::from_secs(1),
                1,
                None,
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(40, 3)].fg, CYAN);
    assert_eq!(
        buffer[(40, 8)].fg,
        MUTED,
        "an empty wait queue is not a warning"
    );
    history = History::default();
    sample.llm_active_requests = None;
    for _ in 0..40 {
        history.observe(&sample, 80);
    }
    terminal
        .draw(|frame| {
            queue(
                frame,
                frame.area(),
                &history,
                Duration::from_secs(1),
                1,
                None,
            )
        })
        .unwrap();
    assert_eq!(terminal.backend().buffer()[(40, 8)].symbol(), "━");
    assert_eq!(terminal.backend().buffer()[(40, 8)].fg, MUTED);
}

#[test]
fn queue_large_ticks_fit_their_gutter_without_truncating_or_covering_the_trace() {
    for value in [2_000, 20_000, u64::MAX] {
        let mut history = History::default();
        let mut sample = live();
        sample.llm_active_requests = Some(value);
        for _ in 0..44 {
            history.observe(&sample, 80);
        }
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(44, 10)).unwrap();
        terminal
            .draw(|frame| {
                queue(
                    frame,
                    frame.area(),
                    &history,
                    Duration::from_secs(1),
                    1,
                    None,
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let (gutter, ceiling, _) = queue_axis(&history, 42, 1, None);
        let label = queue_tick(ceiling);
        let rendered: String = (1..=gutter).map(|x| buffer[(x, 2)].symbol()).collect();
        assert_eq!(rendered.trim_end(), label);
        assert!(
            gutter as usize > label.len(),
            "leave a gap before the trace"
        );
        assert!((2..8).any(|y| buffer[(42, y)].fg == CYAN));
        assert_eq!(buffer[(42, 8)].fg, MUTED);
    }
}

#[test]
fn queue_range_and_time_window_use_the_actual_graph_width_after_reserving_ticks() {
    for (zoom, recent) in [(1, 24), (2, 12)] {
        let mut history = History::default();
        let mut sample = live();
        sample.llm_active_requests = Some(10_000);
        history.observe(&sample, 80);
        sample.llm_active_requests = Some(1);
        for _ in 0..recent {
            history.observe(&sample, 80);
        }
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(30, 10)).unwrap();
        terminal
            .draw(|frame| {
                queue(
                    frame,
                    frame.area(),
                    &history,
                    Duration::from_secs(1),
                    zoom,
                    None,
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let (gutter, ceiling, visible) = queue_axis(&history, 28, zoom, None);
        assert_eq!(visible, (28 - gutter).div_ceil(zoom) as usize);
        assert_eq!(ceiling, 2, "off-screen spike must not flatten the trace");
        let subtitle: String = (1..29).map(|x| buffer[(x, 1)].symbol()).collect();
        // Expanded, the queue names its window; the axis ticks give the range.
        assert!(subtitle.contains(&format!("{visible}s")), "{subtitle}");
        assert!(!subtitle.contains("auto"), "{subtitle}");
        assert_eq!(buffer[(28, 5)].fg, CYAN);
    }
}

/// A live sample from `provider` with these running requests: id, tokens out, own speed.
fn running(provider: &str, waiting: u64, requests: &[(&str, u64, Option<f64>)]) -> Sample {
    let mut sample = live();
    sample.llm_provider = provider.into();
    sample.llm_active_requests = Some(requests.len() as u64);
    sample.llm_waiting_requests = Some(waiting);
    sample.llm_requests = requests
        .iter()
        .map(|(id, out, tps)| domain::RequestUsage {
            provider: provider.into(),
            model: "test".into(),
            id: (*id).into(),
            prompt: 20,
            cached: None,
            output: Some(*out),
            output_tps: *tps,
            completed: false,
            queued: false,
            prefilled: None,
            ttft_ms: None,
            observed_at: Some(SystemTime::now()),
        })
        .collect();
    sample
}

/// The queue panel as text, on Overview (`Some` window) or enlarged (`None`).
fn queue_text(history: &History, height: u16, window: Option<usize>) -> String {
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(44, height)).unwrap();
    terminal
        .draw(|frame| {
            queue(
                frame,
                frame.area(),
                history,
                Duration::from_secs(1),
                1,
                window,
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| (0..44).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn overview_queue_lists_each_running_request_with_its_own_rate() {
    // oMLX reports a speed for every generating request; mlx-serve does the same.
    for provider in ["oMLX", "mlx-serve"] {
        let mut history = History::default();
        history.observe(
            &running(
                provider,
                1,
                &[("105", 1742, Some(41.0)), ("106", 918, Some(54.7))],
            ),
            10,
        );
        let roomy = queue_text(&history, 5, Some(60));
        for label in [
            "2 active · 1 waiting",
            "● GEN     105 1.7k out",
            "41.0 tok/s│",
            "● GEN     106 918 out",
            "54.7 tok/s│",
        ] {
            assert!(
                roomy.contains(label),
                "{provider}: missing {label}\n{roomy}"
            );
        }
        // One row of room: the bottom border says another request is hidden.
        let tight = queue_text(&history, 3, Some(60));
        assert!(tight.contains("+1 more"), "{tight}");
        assert!(tight.contains("105 1.7k out"), "{tight}");
        let tall = queue_text(&history, 10, Some(60));
        assert!(
            tall.contains("106 918 out") && !tall.contains("more"),
            "{tall}"
        );
        // Enlarged, the requests sit above the history, which keeps the counts.
        let enlarged = queue_text(&history, 16, None);
        assert!(
            enlarged.contains("41.0 tok/s") && enlarged.contains("━ active"),
            "{enlarged}"
        );
        assert_eq!(enlarged.matches("waiting 1").count(), 1, "{enlarged}");
        // A short enlarged panel keeps the history.
        let short = queue_text(&history, 10, None);
        assert!(!short.contains("tok/s"), "{short}");
    }
}

#[test]
fn queue_rows_show_each_phase_with_its_own_indicator() {
    let mut sample = running(
        "oMLX",
        1,
        &[
            ("gen-a1", 1400, Some(25.8)),
            ("pre-b2", 0, None),
            ("que-c3", 0, None),
        ],
    );
    sample.llm_requests[1].prompt = 35_628;
    sample.llm_requests[1].prefilled = Some(35_616);
    sample.llm_requests[2].prompt = 8_200;
    sample.llm_requests[2].queued = true;
    sample.llm_active_requests = Some(2);
    let mut history = History::default();
    history.observe(&sample, 10);
    let text = queue_text(&history, 6, Some(60));
    let rows: Vec<&str> = text.lines().collect();
    // The prefill leads even though the runtime lists it second, and it
    // reads 99% while tokens remain: unfinished work never shows 100%.
    assert!(rows[1].contains("◐ PREFILL pre-b2 35.6k"), "{text}");
    assert!(
        rows[1].ends_with(" 99%│") && !text.contains("100%"),
        "{text}"
    );
    assert!(rows[2].contains("● GEN     gen-a1 1.4k out"), "{text}");
    assert!(rows[3].contains("○ QUEUED  que-c3 8.2k in"), "{text}");
    // Readings share the right edge.
    assert!(rows[2].ends_with("25.8 tok/s│"), "{text}");

    // Narrow: the id goes first; the state, bar and reading stay.
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(30, 5)).unwrap();
    terminal
        .draw(|frame| {
            queue(
                frame,
                frame.area(),
                &history,
                Duration::from_secs(1),
                1,
                Some(60),
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let line = |y: u16| (0..30).map(|x| buffer[(x, y)].symbol()).collect::<String>();
    assert!(line(0).contains("●2 ○1"), "{}", line(0));
    assert!(
        line(1).starts_with("│◐ PREFILL 35.6k") && line(1).ends_with("99%│"),
        "{}",
        line(1)
    );
    assert!(
        !line(2).contains("gen-a1") && line(2).ends_with("25.8 tok/s│"),
        "{}",
        line(2)
    );
    // Marks carry state colour; the words beside them use text colours.
    assert_eq!(buffer[(1, 1)].fg, CYAN);
    assert_eq!(buffer[(1, 3)].fg, YELLOW);
    assert_eq!(buffer[(3, 3)].fg, Color::White);
}

#[test]
fn a_request_without_its_own_rate_keeps_its_row() {
    // Splash measures a request's speed only while it runs alone.
    let mut history = History::default();
    history.observe(&running("oMLX", 0, &[("a1", 300, Some(38.2))]), 10);
    // One request is listed in full by prompt load; the queue keeps counts.
    assert!(!queue_text(&history, 8, Some(60)).contains("38.2 tok/s"));
    history.observe(
        &running("oMLX", 0, &[("a1", 340, None), ("b2", 4, None)]),
        10,
    );
    let both = queue_text(&history, 8, Some(60));
    for label in ["a1 340 out", "b2 4 out"] {
        assert!(both.contains(label), "missing {label}\n{both}");
    }
    assert_eq!(both.matches("—│").count(), 2, "{both}");
    // A runtime that never reports one keeps the usual queue.
    let mut vllm = History::default();
    vllm.observe(&running("vLLM", 0, &[("x", 30, None), ("y", 40, None)]), 10);
    let usual = queue_text(&vllm, 8, Some(60));
    assert!(
        !usual.contains("—") && usual.contains("━ active"),
        "{usual}"
    );
}

#[test]
fn compact_queue_keeps_exact_counts_when_there_is_no_room_for_a_scale() {
    let mut history = History::default();
    let mut sample = live();
    sample.llm_active_requests = Some(1234);
    sample.llm_waiting_requests = Some(5678);
    history.observe(&sample, 80);
    for height in [2, 3, 4, 5] {
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(20, height)).unwrap();
        terminal
            .draw(|frame| {
                queue(
                    frame,
                    frame.area(),
                    &history,
                    Duration::from_secs(1),
                    1,
                    None,
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text = (0..height)
            .map(|y| (0..20).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("active 1234"), "height {height}: {text}");
        assert!(text.contains("waiting 5678"), "height {height}: {text}");
        assert!(text.contains("queue"), "height {height}: {text}");
        assert!(!text.contains("0–"), "a compact summary has no fake axis");
        assert!(!text.contains("Enter: history"), "no filler line");
    }
}

#[test]
fn six_row_queue_distinguishes_one_request_from_the_two_request_tick() {
    let mut history = History::default();
    let mut sample = live();
    sample.llm_active_requests = Some(1);
    sample.llm_waiting_requests = Some(0);
    history.observe(&sample, 80);
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(50, 6)).unwrap();
    terminal
        .draw(|frame| {
            queue(
                frame,
                frame.area(),
                &history,
                Duration::from_secs(1),
                1,
                None,
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(1, 2)].symbol(), "2");
    assert_ne!(buffer[(48, 2)].fg, CYAN);
    assert_eq!(buffer[(48, 3)].fg, CYAN);
    assert_eq!(buffer[(48, 4)].fg, MUTED);
}

#[test]
fn waiting_requests_turn_the_waiting_series_yellow() {
    let mut history = History::default();
    let mut sample = live();
    sample.llm_active_requests = Some(2);
    sample.llm_waiting_requests = Some(0);
    history.observe(&sample, 80);
    sample.llm_waiting_requests = Some(1);
    history.observe(&sample, 80);
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(50, 8)).unwrap();
    terminal
        .draw(|frame| {
            queue(
                frame,
                frame.area(),
                &history,
                Duration::from_secs(1),
                1,
                None,
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert!(buffer.content.iter().any(|cell| cell.fg == YELLOW));
    let legend: String = (0..50).map(|x| buffer[(x, 7)].symbol()).collect();
    assert!(legend.contains("━ active  ━ waiting  ═ equal"), "{legend}");
}

#[test]
fn narrow_queue_preserves_both_counts_and_complete_window_labels() {
    for (active, waiting) in [(1, 0), (1234, 5678)] {
        let mut sample = live();
        sample.llm_active_requests = Some(active);
        sample.llm_waiting_requests = Some(waiting);
        let mut history = History::default();
        for _ in 0..40 {
            history.observe(&sample, 80);
        }
        let text = super::super::tests::render_view(20, 8, |frame| {
            queue(
                frame,
                frame.area(),
                &history,
                Duration::from_secs(1),
                1,
                None,
            );
        });
        assert!(text.contains(&format!("active {active}")), "{text}");
        assert!(text.contains(&format!("waiting {waiting}")), "{text}");
        if active == 1 {
            assert!(text.contains("window 15s"), "{text}");
        }
    }
}

#[test]
fn trace_preserves_gaps_boundaries_and_overflow() {
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(5, 5)).unwrap();
    terminal
        .draw(|frame| {
            trace(
                frame,
                frame.area(),
                &[Some(0), None, Some(16), Some(0), Some(u64::MAX)],
                &[false, false, false, true, false],
                16,
                CYAN,
                1,
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(1, 2)].symbol(), " ");
    assert_eq!(buffer[(3, 2)].symbol(), " ");
    assert_eq!(buffer[(4, 0)].symbol(), "↑");
}

#[test]
fn queue_crossings_and_rounded_unequal_values_do_not_claim_equality() {
    for pairs in [vec![(0, 1), (1, 0)], vec![(100, 101), (100, 101)]] {
        let mut history = History::default();
        for (active, waiting) in pairs {
            let mut sample = live();
            sample.llm_active_requests = Some(active);
            sample.llm_waiting_requests = Some(waiting);
            history.observe(&sample, 80);
        }
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(44, 10)).unwrap();
        terminal
            .draw(|frame| {
                queue(
                    frame,
                    frame.area(),
                    &history,
                    Duration::from_secs(1),
                    1,
                    None,
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert!((2..9)
            .flat_map(|y| (4..43).map(move |x| (x, y)))
            .all(|xy| buffer[xy].symbol() != "═"));
        assert!((2..9)
            .flat_map(|y| (4..43).map(move |x| (x, y)))
            .any(|xy| matches!(buffer[xy].symbol(), "┳" | "┻" | "≈")));
    }
}

#[test]
fn shared_queue_window_uses_the_same_samples_at_different_widths() {
    let mut history = History::default();
    for i in 0..30 {
        let mut sample = live();
        sample.llm_active_requests = Some(if i == 25 { 4 } else { 0 });
        history.observe(&sample, 80);
    }
    for width in [44, 90] {
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(width, 10)).unwrap();
        terminal
            .draw(|frame| {
                queue(
                    frame,
                    frame.area(),
                    &history,
                    Duration::from_secs(1),
                    1,
                    Some(10),
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        // On the Overview the top bar names the shared window; the plot
        // starts on the first row inside the border.
        let heading: String = (1..width - 1).map(|x| buffer[(x, 1)].symbol()).collect();
        assert!(!heading.contains("10s"), "{heading}");
        let graph_start = 4;
        let graph_width = width - 1 - graph_start;
        let spike_start = graph_start + graph_width.div_ceil(2);
        assert!((1..8).any(|y| buffer[(spike_start, y)].fg == CYAN));
        assert!((1..8).all(|y| buffer[(width - 2, y)].fg != CYAN));
    }
}

#[test]
fn swapping_active_and_waiting_draws_clean_connectors_without_ladder_ticks() {
    let mut history = History::default();
    for i in 0..34 {
        let mut sample = live();
        sample.llm_active_requests = Some(if i == 14 { 0 } else { 1 });
        sample.llm_waiting_requests = Some(if i == 14 { 1 } else { 0 });
        history.observe(&sample, 80);
    }
    for (width, height) in [(44, 12), (80, 20)] {
        let mut terminal =
            Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                queue(
                    frame,
                    frame.area(),
                    &history,
                    Duration::from_secs(1),
                    1,
                    Some(34),
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let junction_columns: Vec<_> = (4..width - 1)
            .filter(|x| (2..height - 1).any(|y| buffer[(*x, y)].symbol() == "┳"))
            .collect();
        assert_eq!(junction_columns.len(), 2, "one departure and one return");
        for x in junction_columns {
            let top = (2..height - 1)
                .find(|y| buffer[(x, *y)].symbol() == "┳")
                .unwrap();
            let bottom = (top + 1..height - 1)
                .find(|y| buffer[(x, *y)].symbol() == "┻")
                .unwrap();
            for y in top + 1..bottom {
                assert_eq!(
                    buffer[(x, y)].symbol(),
                    "┃",
                    "connector must not look like ticks"
                );
                assert_eq!(buffer[(x, y)].fg, MUTED);
            }
        }
        assert!((2..height - 1)
            .flat_map(|y| (4..width - 1).map(move |x| (x, y)))
            .all(|xy| !matches!(buffer[xy].symbol(), "┼" | "═")));
    }
}

#[test]
fn compact_queue_marks_each_request_running_or_waiting() {
    assert_eq!(request_icons(Some(3), '●', 10), "●●●");
    assert_eq!(request_icons(Some(0), '○', 10), "");
    assert_eq!(request_icons(None, '○', 10), "");
    assert_eq!(
        request_icons(Some(40), '○', 5),
        "○○○○+",
        "a long queue ends in +"
    );
    assert_eq!(request_icons(Some(5), '○', 0), "+");

    let mut history = History::default();
    let mut sample = live();
    sample.llm_active_requests = Some(1);
    sample.llm_waiting_requests = Some(5);
    history.observe(&sample, 80);
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(40, 4)).unwrap();
    terminal
        .draw(|frame| {
            queue(
                frame,
                frame.area(),
                &history,
                Duration::from_secs(1),
                1,
                None,
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let line = |y: u16| (0..40).map(|x| buffer[(x, y)].symbol()).collect::<String>();
    assert!(line(1).contains("active 1  ●"), "{}", line(1));
    assert!(line(2).contains("waiting 5 ○○○○○"), "{}", line(2));
    // Icons sit in one column for both rows, coloured by their state.
    let x = line(2).chars().position(|c| c == '○').unwrap() as u16;
    assert_eq!(buffer[(x, 1)].symbol(), "●");
    assert_eq!(buffer[(x, 1)].fg, CYAN);
    assert_eq!(buffer[(x, 2)].fg, YELLOW);

    // Three rows: the waiting count and its marks move into the border.
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(40, 3)).unwrap();
    terminal
        .draw(|frame| {
            queue(
                frame,
                frame.area(),
                &history,
                Duration::from_secs(1),
                1,
                None,
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let bottom = (0..40).map(|x| buffer[(x, 2)].symbol()).collect::<String>();
    assert!(bottom.contains(" waiting 5 ○○○○○ "), "{bottom}");
}

#[test]
fn first_token_bars_are_solid_with_eighth_block_tops() {
    let mut history = History::default();
    for (id, ttft) in [("a", 400), ("b", 300)] {
        let mut sample = live();
        sample.llm_requests.push(domain::RequestUsage {
            provider: "oMLX".into(),
            model: "test".into(),
            id: id.into(),
            prompt: 100,
            cached: None,
            output: None,
            completed: true,
            observed_at: Some(SystemTime::now()),
            queued: false,
            prefilled: None,
            ttft_ms: Some(ttft),
            output_tps: None,
        });
        history.observe(&sample, 10);
    }
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(20, 10)).unwrap();
    terminal
        .draw(|frame| latency(frame, frame.area(), &history, 1))
        .unwrap();
    let buffer = terminal.backend().buffer();
    // Newest at the right: 300 ms, then 400 ms; full cells are filled backgrounds.
    let height = |x: u16| (0..10).filter(|y| buffer[(x, *y)].bg == BLUE).count();
    assert!(height(17) > height(18), "{} vs {}", height(17), height(18));
    assert!(
        (0..10).all(|y| buffer[(18, y)].symbol() != "▇"),
        "no striped glyph cells"
    );
}
