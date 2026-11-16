// SPDX-License-Identifier: MIT
//! The served model's facts in Diagnostics, judged as every screen is.
use super::*;
use crate::model_facts::ModelFacts;

/// What the owner's Mac Studio serves: Qwen3.8-27B-oQ4e-mtp through
/// Splash with int8 KV and a DFlash2 draft.
fn owner_facts() -> ModelFacts {
    ModelFacts {
        bytes_per_pass: Some(15_020_479_488),
        parameters: Some(26_895_998_464),
        active_parameters: Some(25_624_600_064),
        bits_per_weight: Some(4.6894),
        quantization: Some("4/5-bit g64".into()),
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
        loaded_bytes: Some(25_744_836_152),
        estimated_bytes: Some(17_820_265_635),
        ..ModelFacts::default()
    }
}

#[test]
fn diagnostics_with_model_facts_pass_the_qa_judge() {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    app.diagnostics_open = true;
    let plain = text(&draw(&app, 120, 40)).join("\n");
    assert!(
        !plain.contains("read per pass"),
        "no facts, no lines:\n{plain}"
    );
    app.collector.current.model = owner_facts();
    let case = Case {
        name: "diagnostics-model-facts",
        impact: false,
        app,
    };
    let mut failures = Vec::new();
    for (width, height) in SIZES {
        for problem in judge(&case, width, height) {
            failures.push(format!("{width}×{height}: {problem}"));
        }
        // Each fact once, on the first page, its words whole: a line the
        // panel wraps breaks between words, so with its rows run together
        // every fact reads as written.
        let screen = text(&draw(&case.app, width, height))
            .join(" ")
            .replace('│', " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for fact in [
            "14.0 GiB read per pass",
            "4.69 bits per weight, 4/5-bit g64",
            "KV 32.5 KiB per token, int8",
            "152.6 MiB state per sequence",
            "draft 3.6 GiB",
            "Version: oMLX 0.7.3 · Splash 1.5.0",
        ] {
            if screen.matches(fact).count() != 1 {
                failures.push(format!("{width}×{height}: {fact:?} not shown once"));
            }
        }
    }
    assert!(failures.is_empty(), "QA judge:\n{}", failures.join("\n"));
}
