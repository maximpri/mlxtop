// SPDX-License-Identifier: MIT
//! The QA judge on the slowdown reasons and next steps added after usual
//! speed was judged per verify pass: a full KV cache, and nothing else
//! unusual judged per pass. Each on the strip, the generation chart and in
//! Diagnostics, at every size.
use super::qa::{judge, Case, SIZES};
use super::*;

const KV_FULL: &str = "The KV cache was full, so requests were recomputed: give the runtime more KV memory, or run fewer requests at once.";
const PER_PASS: &str = "Nothing on the machine explains it, and it is slower per verify pass, so not the text: the runtime or its draft model; check the runtime's log at this time.";

/// The dashboard fixture slowed for `cause`, as the engine leaves it: its
/// insight, and its samples marked on the generation chart.
fn slowed(cause: CorrelationCause, lead: &str) -> App {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    let reasons: Vec<CorrelationCause> = (cause != CorrelationCause::None)
        .then_some(cause)
        .into_iter()
        .collect();
    app.collector.current.correlation = CorrelationInsight {
        direction: ThroughputDirection::Down,
        cause,
        lead: lead.into(),
        summary: if lead.is_empty() {
            "GEN ↓50.0% (50.0→25.0 tok/s) · nothing else unusual".into()
        } else {
            format!("GEN ↓50.0% (50.0→25.0 tok/s) · unusual: {lead}")
        },
        delta_percent: Some(-50.0),
        slow: true,
        unusual: reasons.clone(),
        notes: reasons.iter().map(|_| lead.to_owned()).collect(),
        episode: Some(domain::DropEpisode {
            reference: 50.0,
            low: 25.0,
            seconds: 6.0,
            samples: 6,
        }),
        usual_speed: Some((50.0, 40.0)),
        ..CorrelationInsight::default()
    };
    for point in app.collector.generation_history.iter_mut().rev().take(3) {
        if point.value.is_some() {
            point.tone = Tone::Unusual;
        }
    }
    app
}

fn kv_full() -> App {
    slowed(
        CorrelationCause::KvPreemption,
        "KV cache full · 2 requests recomputed (usual 0)",
    )
}

/// Nothing else unusual, judged per verify pass.
fn per_pass() -> App {
    let mut app = slowed(CorrelationCause::None, "");
    app.collector.current.llm_tokens_per_pass = Some(5.5);
    app
}

fn open(mut app: App) -> App {
    app.diagnostics_open = true;
    app
}

fn cases() -> Vec<Case> {
    let case = |name, app| Case {
        name,
        impact: true,
        app,
    };
    vec![
        case("kv-full-slowdown", kv_full()),
        case("kv-full-diagnostics", open(kv_full())),
        case("per-pass-nothing-else", per_pass()),
        case("per-pass-nothing-else-diagnostics", open(per_pass())),
    ]
}

#[test]
fn a_full_kv_cache_and_per_pass_advice_pass_the_qa_judge_at_every_size() {
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
fn a_full_kv_cache_and_per_pass_advice_read_where_a_slowdown_does() {
    // The next step whole where the strip has room, the reason under the
    // generation chart's bars where they have room for it.
    let wide = render_app(&kv_full(), 200, 60);
    assert!(wide.contains(&format!("→ {KV_FULL}")), "{wide}");
    assert!(render_app(&kv_full(), 120, 40).contains("▼ KV cache full"));
    let narrow = render_app(&kv_full(), 80, 24);
    assert!(
        narrow.contains("→ The KV cache was full, so requests were recomputed:"),
        "{narrow}"
    );
    // Diagnostics says what was unusual and what to change.
    let diagnostics = render_app(&open(kv_full()), 200, 60);
    for line in [
        "WITH KV cache full · 2 requests recomputed (usual 0)",
        "Unusual: KV cache full · 2 requests recomputed (usual 0)",
    ] {
        assert!(diagnostics.contains(line), "{line}\n{diagnostics}");
    }
    // Nothing else unusual, judged per verify pass: not the text.
    let pass = render_app(&per_pass(), 200, 60);
    assert!(pass.contains(&format!("→ {PER_PASS}")), "{pass}");
}
