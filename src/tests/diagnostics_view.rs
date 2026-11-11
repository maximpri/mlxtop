// SPDX-License-Identifier: MIT
use super::*;
#[test]
fn wrapping_fits_cells_and_preserves_long_words() {
    let text = vec![
        "Some context with a verylongendpointwithoutspaces".into(),
        String::new(),
        "大きいGPU 🙂 status".into(),
    ];
    for width in [12, 24, 80] {
        let lines = wrapped(&text, width);
        assert!(lines.iter().all(|line| line.width() <= width));
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("verylongendpointwithoutspaces"));
    }
}

#[test]
fn diagnostics_navigation_scrolls_without_changing_charts_or_polling() {
    use crate::test_support::*;
    use crate::tests::{populate_dashboard_fixture, render_app, test_app};
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    app.collector.current.runtime.provider = Some("Ollama".into());
    let press = |app: &mut App, code| app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    press(&mut app, KeyCode::Char('d'));
    for (w, h) in [(80, 24), (120, 40), (180, 50)] {
        let screen = render_app(&app, w, h);
        assert!(screen.contains("Diagnostics") && screen.contains("ASSESSMENT"));
        assert!(screen.contains("Endpoint:"));
    }
    render_app(&app, 80, 24);
    let chart = app.charts.focused;
    press(&mut app, KeyCode::End);
    assert!(app.diagnostics_scroll > 0);
    assert!(render_app(&app, 80, 24).contains("record requests"));
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::PageUp);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::PageDown);
    assert_eq!(app.charts.focused, chart);
    press(&mut app, KeyCode::Char('p'));
    assert!(app.paused);
    press(&mut app, KeyCode::Home);
    assert!(render_app(&app, 80, 24).contains("PAUSED"));
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.tab, 1);
    assert!(!app.diagnostics_open);
    app.top_filtering = true;
    press(&mut app, KeyCode::Char('d'));
    assert_eq!(app.top_filter, "d");
    assert!(!app.diagnostics_open);
    app.top_filtering = false;
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Esc);
    assert!(!app.quit && !app.diagnostics_open);
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Char('q'));
    assert!(app.quit);
}

#[test]
fn summary_shows_evidence_without_turning_gpu_load_into_a_bottleneck() {
    use crate::tests::{populate_dashboard_fixture, render_app, test_app};
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    app.collector.current.gpu_util = Some(99);
    app.collector.current.correlation = Default::default();
    for (w, h) in [(80, 24), (120, 40), (180, 50)] {
        let screen = render_app(&app, w, h);
        assert!(screen.contains("✓ Healthy · no bottleneck"), "{screen}");
        // A healthy verdict leaves the numbers to the panels below it.
        assert!(!screen.contains("no bottleneck · GPU 99%"), "{screen}");
        assert!(!screen.contains("GPU busy"));
        assert!(screen.contains("d diagnostics"));
        assert!(screen.contains("prompt load") && screen.contains("paging"));
    }
    app.collector.current.llm_remote = true;
    assert!(render_app(&app, 80, 24).contains("Local host:"));
    assert!(render_app(&app, 80, 24)
        .contains("Remote inference is not correlated with local hardware."));
    app.collector.current.pressure = "RED".into();
    assert!(render_app(&app, 80, 24).contains("Local host: Memory bottleneck"));
}

#[test]
fn advice_that_does_not_fit_ends_at_a_word() {
    assert_eq!(fit_words("close other apps", 40), "close other apps");
    assert_eq!(
        fit_words("The model no longer fits in RAM: close other apps", 34),
        "The model no longer fits in RAM…"
    );
    assert_eq!(fit_words("unbreakable", 4), "…");
}
