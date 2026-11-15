// SPDX-License-Identifier: MIT
//! What Splash and oMLX say about the draft and what held work up, in the request panel and
//! Diagnostics, judged as every screen is.
use super::*;
use crate::model_facts::ModelFacts;

/// The owner's Mac Studio: oMLX runs Qwen3.8-27B-oQ4e-mtp through Splash with a DFlash2 draft,
/// and keeps Splash's status to itself.
fn omlx_through_splash() -> App {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    app.collector.current.llm_engine = Some("Splash".into());
    app.collector.current.model = ModelFacts {
        bytes_per_pass: Some(15_020_479_488),
        kv_dtype: Some("int8".into()),
        kv_bytes_per_token_reported: Some(33_280),
        fixed_state_bytes: Some(160_038_912),
        context_configured: Some(131_072),
        context_trained: Some(262_144),
        draft: Some(true),
        draft_bytes: Some(3_848_808_960),
        runtime_version: Some("oMLX 0.7.3 · Splash 1.5.0".into()),
        ..ModelFacts::default()
    };
    app
}

/// What held Splash's work up in the last interval, as its details say it.
const HOLDUPS: [&str; 4] = [
    "1 waiting 3s for 2.0 GiB more memory",
    "2 Metal pipelines compiled while serving",
    "decode paused 3× for prefill (420 ms), 1× for memory (80 ms)",
    "first token 5.1 s: 3.4 s behind another prefill, 1.2 s own prefill, 500 ms host and queue",
];

/// Splash on its own, decoding one request with its draft.
fn splash_alone() -> App {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    let sample = &mut app.collector.current;
    sample.runtime.provider = Some("Splash".into());
    sample.runtime.endpoint = "http://127.0.0.1:8000".into();
    sample.llm_provider = "Splash".into();
    sample.llm_tokens_per_pass = Some(7.8);
    let mut details = HOLDUPS.to_vec();
    details.extend([
        "DFlash draft acceptance 65%",
        "server TTFT p50 2705 ms · p95 28949 ms",
    ]);
    sample.llm_details = Some(details.join(" · "));
    for request in &mut sample.llm_requests {
        request.provider = "Splash".into();
    }
    for entry in &mut app.collector.request_history.entries {
        entry.usage.provider = "Splash".into();
    }
    app
}

/// A screen's words in reading order, as one line: a line the panel wraps breaks between
/// words, so with its rows run together every fact reads as written.
fn words(app: &App, width: u16, height: u16) -> String {
    text(&draw(app, width, height))
        .join(" ")
        .replace(['│', '║'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn judged(name: &'static str, app: App, failures: &mut Vec<String>) -> App {
    let case = Case {
        name,
        impact: false,
        app,
    };
    for (width, height) in SIZES {
        for problem in judge(&case, width, height) {
            failures.push(format!("{name} at {width}×{height}: {problem}"));
        }
    }
    case.app
}

#[test]
fn diagnostics_say_what_held_splash_up_and_why_omlx_hides_the_draft() {
    let mut failures = Vec::new();
    let mut hosted = omlx_through_splash();
    hosted.diagnostics_open = true;
    let hosted = judged("diagnostics-omlx-through-splash", hosted, &mut failures);
    let mut alone = splash_alone();
    alone.diagnostics_open = true;
    let alone = judged("diagnostics-splash-holdups", alone, &mut failures);
    let draft = "Draft: tokens per verify pass unknown · oMLX keeps Splash's status private";
    for (width, height) in SIZES {
        // Each said once, on the first page, its words whole.
        let screen = words(&hosted, width, height);
        if screen.matches(draft).count() != 1 {
            failures.push(format!("{width}×{height}: the draft line not shown once"));
        }
        let screen = words(&alone, width, height);
        for fact in HOLDUPS {
            if screen.matches(fact).count() != 1 {
                failures.push(format!("{width}×{height}: {fact:?} not shown once"));
            }
        }
        // Splash on its own explains nothing it does not hide.
        if screen.contains("Draft:") {
            failures.push(format!("{width}×{height}: a draft line for Splash alone"));
        }
    }
    assert!(failures.is_empty(), "QA judge:\n{}", failures.join("\n"));
}

#[test]
fn the_request_panel_shows_tokens_per_pass_beside_the_live_output() {
    let mut failures = Vec::new();
    let alone = judged("request-tokens-per-pass", splash_alone(), &mut failures);
    let hosted = judged(
        "request-omlx-through-splash",
        omlx_through_splash(),
        &mut failures,
    );
    let mut shared = splash_alone();
    let mut other = shared.collector.current.llm_requests[0].clone();
    other.id = "other".into();
    shared.collector.current.llm_requests.push(other);
    let shared = judged("request-two-writers", shared, &mut failures);
    for (width, height) in SIZES {
        let screen = words(&alone, width, height);
        if screen.matches("OUT 1,200 · 7.8 tok/pass").count() != 1 {
            failures.push(format!(
                "{width}×{height}: tokens per pass not beside the output"
            ));
        }
        // Unknown under oMLX; shared between two writers, it is neither request's.
        for (name, app) in [("oMLX", &hosted), ("two writers", &shared)] {
            if words(app, width, height).contains("tok/pass") {
                failures.push(format!("{width}×{height}: tokens per pass with {name}"));
            }
        }
    }
    assert!(failures.is_empty(), "QA judge:\n{}", failures.join("\n"));
}
