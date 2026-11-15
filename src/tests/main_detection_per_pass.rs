// SPDX-License-Identifier: MIT
//! Usual speed compared like with like: per verify pass when a draft's
//! tokens per pass are counted, and at the same depth when the model's facts
//! say what a pass reads. A draft that lands fewer tokens a pass is the text
//! and a deeper context reads more cache: neither is a slowdown. What slows
//! the machine still is, with its reason.
use super::*;
use crate::model_facts::ModelFacts;

/// The owner's server's model, as its facts give it: 15,020,479,488 bytes
/// a pass, 33,280 KV bytes a token (16 layers keep int8 KV) and a draft.
fn server_facts() -> ModelFacts {
    ModelFacts {
        bytes_per_pass: Some(15_020_479_488),
        kv_layers: Some(16),
        kv_heads: Some(4),
        head_dim: Some(256),
        kv_bytes_per_element: Some(1.0 + 1.0 / 64.0),
        fixed_state_bytes: Some(160_038_912),
        draft: Some(true),
        ..ModelFacts::default()
    }
}

/// A KV-heavy model without a draft, as Qwen3-30B-A3B at 8-bit: 3.5 GB of
/// active weights a pass and 96 KiB of 16-bit KV a token, so each 16k
/// tokens of context add 1.6 GB to every pass.
fn kv_heavy_facts() -> ModelFacts {
    ModelFacts {
        bytes_per_pass: Some(3_500_000_000),
        experts: Some(128),
        experts_per_token: Some(8),
        kv_layers: Some(48),
        kv_heads: Some(4),
        head_dim: Some(128),
        kv_bytes_per_element: Some(2.0),
        draft: Some(false),
        ..ModelFacts::default()
    }
}

/// One poll of request `id` writing alone at `tps`, `context` tokens deep,
/// on a model with `facts`, its runtime counting `tokens_per_pass`.
fn writing(
    id: &str,
    second: u64,
    tps: f64,
    context: u64,
    facts: &ModelFacts,
    tokens_per_pass: Option<f64>,
) -> Sample {
    let mut sample = paging(second, tps, 0);
    let request = &mut sample.llm_requests[0];
    request.id = id.into();
    request.prompt = context - 200;
    request.output = Some(200);
    sample.model = facts.clone();
    sample.llm_tokens_per_pass = tokens_per_pass;
    sample
}

/// The owner's server switching from thinking off to thinking on, as its
/// request log has them: 123 tok/s at 7.8 tokens a verify pass (draft
/// acceptance 97%), then 88 tok/s at 5.55 (64%), both 15.8 passes a second.
/// `counted`: whether the runtime counts tokens per pass.
fn thinking_switch(counted: bool) -> Vec<Sample> {
    let facts = server_facts();
    let poll = |id: &str, second: u64, tps: f64, tokens: f64, acceptance: f64| {
        let mut sample = writing(id, second, tps, 1_500, &facts, counted.then_some(tokens));
        sample.llm_draft_acceptance = Some(acceptance);
        sample
    };
    let mut samples: Vec<Sample> = (0..20)
        .map(|second| poll("off", second, 123.0, 7.8, 0.97))
        .collect();
    samples.extend((20..40).map(|second| poll("on", second, 88.0, 5.55, 0.64)));
    observe_all(&mut CorrelationEngine::default(), &mut samples);
    samples
}

#[test]
fn a_draft_that_lands_fewer_tokens_a_pass_is_the_text_not_a_slowdown() {
    let samples = thinking_switch(true);
    // The machine runs as many verify passes as before: nothing is slow.
    for sample in &samples {
        let insight = &sample.correlation;
        assert!(
            !insight.slow && !insight.is_material_drop(),
            "{}",
            insight.summary
        );
    }
    // The usual speed is this machine's passes at this text's tokens per
    // pass, for the same work as before.
    let on = &samples[39].correlation;
    let (usual, slow_below) = on.usual_speed.unwrap();
    assert!((usual - 123.0 / 7.8 * 5.55).abs() < 0.1, "{usual}");
    assert!((slow_below - usual * 0.8).abs() < 0.1, "{slow_below}");
    assert_eq!(on.usual_for, "1 request · 1.5k context");
    assert_eq!(samples[19].correlation.usual_speed.unwrap().0, 123.0);
    // The draft's lower acceptance is the text: said in Diagnostics while
    // it is new, never a slowdown's reason.
    let first = &samples[21].correlation;
    assert_eq!(first.unusual, vec![CorrelationCause::DraftAcceptance]);
    assert_eq!(first.notes, vec!["draft acceptance 64% (usual 97%)"]);

    // Judged per token, as when the runtime does not count, the same
    // switch is a slowdown put down to the draft.
    let per_token = thinking_switch(false);
    let slowed = per_token
        .iter()
        .map(|sample| &sample.correlation)
        .find(|insight| insight.is_material_drop())
        .expect("a 28% drop per token is a slowdown");
    assert_eq!(slowed.cause, CorrelationCause::DraftAcceptance);
    assert_eq!(
        slowed.summary,
        "GEN ↓28.5% (123.0→88.0 tok/s) · unusual: draft acceptance 64% (usual 97%)"
    );
    assert_eq!(slowed.usual_for, "1 request · 1.5k context");
}

/// Fewer draft tokens accepted make each pass emit fewer tokens; they do
/// not make the machine run fewer passes. A slowdown in passes a second has
/// another reason, or none that mlxtop can see.
#[test]
fn a_slowdown_judged_per_pass_is_never_put_down_to_the_draft() {
    let facts = server_facts();
    let poll = |second: u64, tps: f64, tokens: f64, acceptance: f64| {
        let mut sample = writing("a", second, tps, 1_500, &facts, Some(tokens));
        sample.llm_draft_acceptance = Some(acceptance);
        sample
    };
    let mut samples: Vec<Sample> = (0..15).map(|s| poll(s, 123.0, 7.8, 0.97)).collect();
    // The draft lands 5.5 tokens a pass, and the machine runs 9.1 passes a
    // second instead of 15.8.
    samples.extend((15..20).map(|s| poll(s, 50.0, 5.5, 0.64)));
    observe_all(&mut CorrelationEngine::default(), &mut samples);
    let slowed = &samples[17].correlation;
    assert!(slowed.is_material_drop(), "{}", slowed.summary);
    assert_eq!(slowed.cause, CorrelationCause::None);
    assert!(slowed.unusual.is_empty(), "{:?}", slowed.unusual);
    // 15.8 passes a second at 5.5 tokens a pass is 86.7 tok/s.
    assert_eq!(
        slowed.summary,
        "GEN ↓42.4% (86.7→50.0 tok/s) · nothing else unusual"
    );
}

#[test]
fn a_second_writer_halves_each_requests_passes_and_is_judged_in_its_own_bucket() {
    let facts = server_facts();
    let mut engine = CorrelationEngine::default();
    let mut observe = |sample: Sample| engine.observe(&sample, defaults());
    let alone = |second: u64| writing("a", second, 123.0, 1_500, &facts, Some(7.8));
    // Two answers share the GPU, each at `each` tok/s with 7.8 tokens a
    // pass: speed is their sum, and passes are counted per request.
    let pair = |second: u64, each: f64| {
        beside(
            writing("a", second, 2.0 * each, 1_500, &facts, Some(7.8)),
            1_300,
            false,
        )
    };
    for second in 0..15 {
        assert!(!observe(alone(second)).slow);
    }
    // A second writer is new work: learned, never judged against one alone,
    // though each answer runs at 57.8 tok/s instead of 123.
    let shared: Vec<CorrelationInsight> =
        (15..30).map(|second| observe(pair(second, 57.8))).collect();
    for insight in &shared {
        assert!(
            !insight.slow && !insight.is_material_drop(),
            "{}",
            insight.summary
        );
    }
    let usual = shared.last().unwrap();
    assert_eq!(usual.usual_for, "2 requests · 3.0k context");
    let (together, _) = usual.usual_speed.unwrap();
    assert!((together - 115.6).abs() < 0.1, "{together}");
    // Each answer runs 7.4 verify passes a second against 15.8 alone: the
    // tokens per pass held, so the passes halved.
    let each = together / 2.0 / 7.8;
    assert!((each / (123.0 / 7.8) - 0.47).abs() < 0.01, "{each}");

    // Slower again within its own bucket is a slowdown, by its third sample.
    let slowed: Vec<CorrelationInsight> =
        (30..33).map(|second| observe(pair(second, 25.0))).collect();
    assert!(!slowed[1].is_material_drop());
    assert!(slowed[2].is_material_drop(), "{}", slowed[2].summary);
    assert_eq!(
        slowed[2].summary,
        "GEN ↓56.7% (115.6→50.0 tok/s) · nothing else unusual"
    );
    // A lone answer is still judged against lone ones.
    observe(alone(33));
    let back = observe(alone(34));
    assert!(!back.slow);
    assert_eq!(back.usual_speed.unwrap().0, 123.0);
    assert_eq!(back.usual_for, "1 request · 1.5k context");
}

/// Each pass reads the weights and the cache of the context: a KV-heavy
/// model twice as deep reads 6.7 GB a pass instead of 5.1, and runs 24%
/// slower with nothing wrong, inside one "context within a factor of two".
#[test]
fn a_deeper_context_that_reads_more_cache_is_not_a_slowdown() {
    let step = |facts: &ModelFacts| {
        let mut samples: Vec<Sample> = (0..20)
            .map(|second| writing("16k", second, 60.0, 16_384, facts, None))
            .collect();
        samples.extend(
            (20..32).map(|second| writing("32k", second, 60.0 * 0.78, 32_768, facts, None)),
        );
        observe_all(&mut CorrelationEngine::default(), &mut samples);
        samples
    };
    let samples = step(&kv_heavy_facts());
    for sample in &samples {
        let insight = &sample.correlation;
        assert!(
            !insight.slow && !insight.is_material_drop(),
            "{}",
            insight.summary
        );
    }
    // The usual speed is the shallower work's, taken at this depth.
    let deep = &samples[21].correlation;
    let at_depth = 60.0 * (3.5e9 + 98_304.0 * 16_384.0) / (3.5e9 + 98_304.0 * 32_768.0);
    let (usual, _) = deep.usual_speed.unwrap();
    assert!((usual - at_depth).abs() < 0.01, "{usual} vs {at_depth}");
    assert!((at_depth / 60.0 - 0.76).abs() < 0.005);
    assert_eq!(deep.usual_for, "1 request · 32.8k context");

    // Without the model's facts the same step reads as a slowdown.
    let unknown = step(&ModelFacts::default());
    assert!(unknown
        .iter()
        .any(|sample| sample.correlation.is_material_drop()));
    // Nor is the depth allowed for on an MoE model with a draft: a pass that
    // verifies several tokens reads the experts each of them routes to.
    let drafted = ModelFacts {
        draft: Some(true),
        ..kv_heavy_facts()
    };
    assert!(step(&drafted)
        .iter()
        .any(|sample| sample.correlation.is_material_drop()));
}

/// On the owner's hybrid model each pass reads 15.0 GB of weights beside a
/// little cache, so a context twice as deep changes little.
#[test]
fn depth_is_allowed_for_by_the_cache_each_token_reads() {
    let facts = server_facts();
    let mut samples: Vec<Sample> = (0..20)
        .map(|second| writing("a", second, 58.0, 40_000, &facts, Some(4.3)))
        .collect();
    samples.push(writing("b", 20, 52.0, 80_000, &facts, Some(4.3)));
    samples.push(writing("b", 21, 52.0, 80_000, &facts, Some(4.3)));
    observe_all(&mut CorrelationEngine::default(), &mut samples);
    let deep = &samples[21].correlation;
    // 15.02 GB + 40k × 33,280 B against 15.02 GB + 80k × 33,280 B.
    let at_depth =
        58.0 * (15_020_479_488.0 + 40_000.0 * 33_280.0) / (15_020_479_488.0 + 80_000.0 * 33_280.0);
    assert!((deep.usual_speed.unwrap().0 - at_depth).abs() < 0.01);
    assert!(!deep.slow);
}

/// A speed measured before the runtime counted tokens per pass cannot be
/// taken per pass: work judged per pass is learned afresh, never compared
/// with speeds whose text is unknown.
#[test]
fn speeds_without_a_count_are_not_compared_per_pass() {
    let facts = server_facts();
    let mut samples: Vec<Sample> = (0..20)
        .map(|second| writing("a", second, 123.0, 1_500, &facts, None))
        .collect();
    samples.extend((20..32).map(|second| writing("a", second, 60.0, 1_500, &facts, Some(3.8))));
    observe_all(&mut CorrelationEngine::default(), &mut samples);
    for sample in &samples[20..] {
        assert!(!sample.correlation.slow, "{}", sample.correlation.summary);
    }
    assert_eq!(samples[27].correlation.usual_speed, None);
    assert_eq!(samples[28].correlation.usual_speed.unwrap().0, 60.0);
    // Without a count again, every earlier speed is compared per token.
    let mut engine = CorrelationEngine::default();
    let mut last = CorrelationInsight::default();
    for mut sample in samples {
        sample.llm_tokens_per_pass = None;
        last = engine.observe(&sample, defaults());
    }
    assert_eq!(last.usual_for, "1 request · 1.5k context");
}

/// Paging, heat and a prompt read beside the answer, each planted on the
/// owner's server judged per verify pass and by depth: each is still found
/// by its third slow sample, with its own reason.
#[test]
fn what_slows_the_machine_is_found_with_its_reason_when_judged_per_pass() {
    let facts = server_facts();
    let plant = |change: fn(&mut Sample)| {
        let poll = |second: u64, tps: f64| {
            let mut sample = writing("a", second, tps, 1_500, &facts, Some(7.8));
            sample.thermal_state.level = Some(ThermalLevel::Nominal);
            sample
        };
        let mut samples: Vec<Sample> = (0..15).map(|second| poll(second, 123.0)).collect();
        samples.extend((15..20).map(|second| {
            let mut sample = poll(second, 60.0);
            change(&mut sample);
            sample
        }));
        observe_all(&mut CorrelationEngine::default(), &mut samples);
        samples[15..]
            .iter()
            .position(|sample| sample.correlation.is_material_drop())
            .map(|at| (at + 1, samples[15 + at].correlation.cause))
    };
    assert_eq!(
        plant(|sample| sample.swap_in = 64 * MIB),
        Some((3, CorrelationCause::Paging))
    );
    assert_eq!(
        plant(|sample| sample.thermal_state.level = Some(ThermalLevel::Heavy)),
        Some((3, CorrelationCause::Thermal))
    );
    assert_eq!(
        plant(|sample| *sample = beside(sample.clone(), 30_000, true)),
        Some((3, CorrelationCause::ConcurrentPrefill))
    );
}

/// What a person sees once the usual speed is judged per pass: the
/// generation chart's edge gives the draft's acceptance and the usual speed
/// at it, Diagnostics the speed it is judged against and the acceptance that
/// is new, and nothing turns yellow while generation keeps its speed.
#[test]
fn a_usual_speed_judged_per_pass_passes_the_qa_judge_at_every_size() {
    use super::qa::{judge, Case, SIZES};
    let last = thinking_switch(true).pop().unwrap();
    let app = |diagnostics: bool| {
        let mut app = test_app(0);
        populate_dashboard_fixture(&mut app);
        let current = &mut app.collector.current;
        current.model = server_facts();
        current.llm_tokens_per_pass = last.llm_tokens_per_pass;
        current.llm_draft_acceptance = last.llm_draft_acceptance;
        current.correlation = CorrelationInsight {
            direction: ThroughputDirection::Flat,
            ..last.correlation.clone()
        };
        app.diagnostics_open = diagnostics;
        app
    };
    let mut failures = Vec::new();
    for (name, open, lines) in [
        (
            "usual-per-pass",
            false,
            vec!["tok/s · draft 64% · usual 87.5"],
        ),
        (
            "usual-per-pass-diagnostics",
            true,
            vec!["Unusual: draft acceptance 64% (usual 97%)"],
        ),
    ] {
        let case = Case {
            name,
            impact: false,
            app: app(open),
        };
        for (width, height) in SIZES {
            for problem in judge(&case, width, height) {
                failures.push(format!("{name} at {width}×{height}: {problem}"));
            }
            // At 80 and 100 columns the edge gives the usual speed's room
            // to the draft's share.
            let screen = render_app(&case.app, width, height);
            for line in lines.iter().filter(|_| open || width >= 120) {
                if screen.matches(line).count() != 1 {
                    failures.push(format!(
                        "{name} at {width}×{height}: {line:?} not shown once\n{screen}"
                    ));
                }
            }
        }
    }
    // The widest screen has room for the work the usual speed is for.
    let wide = render_app(&app(false), 200, 60);
    assert!(
        wide.contains("tok/s · draft 64% · usual 87.5 for 1 request · 1.5k context"),
        "{wide}"
    );
    assert!(failures.is_empty(), "QA judge:\n{}", failures.join("\n"));
}

/// With nothing else unusual, the next step names what is left. Judged per
/// verify pass, how well the draft guesses is already allowed for, so the
/// text is ruled out; judged per token, it is not.
#[test]
fn nothing_else_unusual_rules_out_the_text_only_when_judged_per_pass() {
    let facts = server_facts();
    let poll = |second: u64, tps: f64, tokens: Option<f64>| {
        writing("a", second, tps, 1_500, &facts, tokens)
    };
    let slowdown = |tokens: Option<f64>| {
        let mut samples: Vec<Sample> = (0..15).map(|s| poll(s, 123.0, tokens)).collect();
        samples.extend((15..20).map(|s| poll(s, 50.0, tokens)));
        observe_all(&mut CorrelationEngine::default(), &mut samples);
        samples.swap_remove(17)
    };
    let per_pass = slowdown(Some(7.8));
    assert_eq!(per_pass.correlation.cause, CorrelationCause::None);
    assert_eq!(
        crate::diagnosis::slowdown_next_step(&per_pass),
        "Nothing on the machine explains it, and it is slower per verify pass, so not the text: the runtime or its draft model; check the runtime's log at this time."
    );
    let per_token = slowdown(None);
    assert_eq!(per_token.correlation.cause, CorrelationCause::None);
    assert_eq!(
        crate::diagnosis::slowdown_next_step(&per_token),
        "Nothing on the machine explains it: the text, the draft model or the runtime itself; check the runtime's log at this time."
    );
    // A prompt is not read per verify pass.
    let mut prefill = per_pass.clone();
    prefill.correlation.phase = Phase::Prefill;
    assert!(crate::diagnosis::slowdown_next_step(&prefill).contains("the text, the draft model"));
}
