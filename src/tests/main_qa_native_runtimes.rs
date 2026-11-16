// SPDX-License-Identifier: MIT
//! The QA judge on what vLLM's, SGLang's, TensorFold's, Ollama's and LM
//! Studio's own readings add to the screen: their Diagnostics lines and the
//! runtime details the Processes view shows, at every size, in a normal and
//! a large-value state.
use super::qa::{judge, Case, SIZES};
use super::*;
use crate::model_facts::ModelFacts;

/// The dashboard fixture, serving `provider` with these readings.
fn serving(provider: &str, details: &str, tune: impl FnOnce(&mut Sample)) -> App {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    let sample = &mut app.collector.current;
    sample.runtime.provider = Some(provider.into());
    sample.llm_provider = provider.into();
    sample.llm_details = Some(details.into());
    tune(sample);
    app
}

const VLLM_DETAILS: &str = "throughput: server token deltas · KV occupancy max 62.0% · server \
    mean TTFT 819 ms (cumulative) · since the last poll: queue wait 1.2 s, 24 ms between \
    tokens, 63% of draft tokens accepted";
const SGLANG_DETAILS: &str = "throughput: live decode tokens, prefill at completion · KV \
    occupancy max 40.0% · server mean TTFT 300 ms (cumulative) · since the last poll: queue \
    wait 800 ms, 31 ms between tokens, 4.1k cached tokens evicted · last 30 s: GPU busy 82%, \
    scheduler idle 3%, scheduler CPU 41%";

/// Each state, and the facts it must show once with their words whole.
fn cases() -> Vec<(Case, Vec<&'static str>)> {
    let vllm = serving("vLLM", VLLM_DETAILS, |sample| {
        sample.llm_tokens_per_pass = Some(2.9);
        sample.llm_kv_free_tokens = Some(41_247);
        sample.llm_preemptions_total = Some(4);
        sample.model = ModelFacts {
            kv_dtype: Some("fp8_e4m3".into()),
            draft: Some(true),
            ..ModelFacts::default()
        };
    });
    let sglang = serving("SGLang", SGLANG_DETAILS, |sample| {
        sample.llm_tokens_per_pass = Some(3.1);
        sample.llm_kv_free_tokens = Some(239_830);
        sample.llm_kv_free_bytes = Some(239_830 * 147_456);
        sample.llm_preemptions_total = Some(2);
        sample.model = ModelFacts {
            kv_bytes_per_token_reported: Some(147_456),
            context_configured: Some(40_960),
            draft: Some(true),
            ..ModelFacts::default()
        };
    });
    let ollama = serving(
        "Ollama",
        "1 loaded models · resident VRAM 6.2 GiB · llama on llama.cpp",
        |sample| {
            sample.model = ModelFacts {
                parameters: Some(8_030_261_248),
                quantization: Some("Q4_0".into()),
                kv_layers: Some(32),
                kv_heads: Some(8),
                head_dim: Some(128),
                context_configured: Some(8192),
                context_trained: Some(8192),
                ..ModelFacts::default()
            };
        },
    );
    let lm_studio = serving(
        "LM Studio",
        "1 loaded models · GGUF · weights 16.8 GiB · up to 4 requests at once",
        |sample| {
            sample.model = ModelFacts {
                quantization: Some("Q4_K_M".into()),
                experts_per_token: Some(8),
                context_configured: Some(4096),
                context_trained: Some(262_144),
                ..ModelFacts::default()
            };
        },
    );
    // The largest readings a big server reports.
    let large = serving("TensorFold", "throughput: live decode tokens, prefill at completion · KV occupancy max 99.9% · 1234567 requests refused admission since the server started", |sample| {
        sample.llm_tokens_per_pass = Some(17.4);
        sample.llm_kv_free_tokens = Some(98_765_432);
        sample.llm_preemptions_total = Some(1_234_567);
    });
    let diagnostics = |mut app: App| {
        app.diagnostics_open = true;
        app
    };
    let processes = |mut app: App| {
        app.tab = 1;
        app
    };
    vec![
        (
            Case {
                name: "diagnostics-vllm",
                impact: false,
                app: diagnostics(vllm),
            },
            vec![
                "Speculative decoding: 2.9 tokens per verify pass",
                "KV cache: 41.2k tokens free · 4 requests preempted since the server started",
                "63% of draft tokens accepted",
                "Context: KV fp8_e4m3",
            ],
        ),
        (
            Case {
                name: "diagnostics-sglang",
                impact: false,
                app: diagnostics(sglang),
            },
            vec![
                "Speculative decoding: 3.1 tokens per verify pass",
                "KV cache: 239.8k tokens free (32.9 GiB) · 2 requests retracted since the server started",
                "last 30 s: GPU busy 82%, scheduler idle 3%, scheduler CPU 41%",
                "Context: KV 144.0 KiB per token · 41.0k tokens allowed",
            ],
        ),
        (
            Case {
                name: "diagnostics-ollama",
                impact: false,
                app: diagnostics(ollama),
            },
            vec![
                "Model: Q4_0 · 8.0B parameters",
                "Context: 8.2k tokens allowed of 8.2k trained",
                "llama on llama.cpp",
            ],
        ),
        (
            Case {
                name: "diagnostics-lm-studio",
                impact: false,
                app: diagnostics(lm_studio),
            },
            vec![
                "Model: Q4_K_M",
                "Context: 4.1k tokens allowed of 262.1k trained",
                "up to 4 requests at once",
            ],
        ),
        (
            Case {
                name: "diagnostics-large-values",
                impact: false,
                app: diagnostics(large),
            },
            vec![
                "Speculative decoding: 17.4 tokens per verify pass",
                "KV cache: 98.8M tokens free · 1234567 requests preempted since the server started",
            ],
        ),
        (
            Case {
                name: "processes-sglang-details",
                impact: false,
                app: processes(serving("SGLang", SGLANG_DETAILS, |_| {})),
            },
            Vec::new(),
        ),
    ]
}

/// The screen's text with borders and line breaks gone, so a line the
/// panel wraps reads as written.
fn words(app: &App, width: u16, height: u16) -> String {
    render_app(app, width, height)
        .replace('│', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn the_native_runtimes_readings_pass_the_qa_judge_at_every_size() {
    let mut failures = Vec::new();
    for (case, facts) in cases() {
        for (width, height) in SIZES {
            for problem in judge(&case, width, height) {
                failures.push(format!("{} at {width}×{height}: {problem}", case.name));
            }
            // Each fact once, its words whole. Diagnostics scrolls; a
            // terminal of 30 rows or more shows these on its first page.
            if height < 30 {
                continue;
            }
            let screen = words(&case.app, width, height);
            for fact in &facts {
                let shown = screen.matches(fact).count();
                if shown != 1 {
                    failures.push(format!(
                        "{} at {width}×{height}: {fact:?} shown {shown} times\n{screen}",
                        case.name
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "QA judge:\n{}", failures.join("\n"));
}

#[test]
fn the_engine_line_in_processes_drops_whole_readings_at_every_width() {
    let mut app = serving("SGLang", SGLANG_DETAILS, |_| {});
    app.tab = 1;
    for (width, height) in SIZES {
        let screen = render_app(&app, width, height);
        let rows: Vec<&str> = screen.lines().collect();
        let title = rows
            .iter()
            .position(|row| row.contains("SGLang ENGINE"))
            .unwrap_or_else(|| panic!("{width}×{height}: no engine panel\n{screen}"));
        let shown = rows[title + 1].trim_matches(|c: char| c == '│' || c.is_whitespace());
        // Whole readings, in order: the rest are dropped, never cut.
        let whole = SGLANG_DETAILS.starts_with(shown)
            && (shown == SGLANG_DETAILS || SGLANG_DETAILS[shown.len()..].starts_with(" · "));
        assert!(whole, "{width}×{height}: {shown:?}");
        assert!(
            shown.starts_with("throughput: live decode tokens"),
            "{shown}"
        );
        if width == 80 {
            assert_eq!(
                shown,
                "throughput: live decode tokens, prefill at completion"
            );
        }
    }
    // A first reading wider than the panel is cut at a word.
    let mut long = serving("SGLang", &"long ".repeat(40), |_| {});
    long.tab = 1;
    let screen = render_app(&long, 80, 24);
    assert!(screen.contains(" long long"), "{screen}");
    assert!(screen.contains("long…"), "{screen}");
}
