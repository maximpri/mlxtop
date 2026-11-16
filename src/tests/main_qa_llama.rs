// SPDX-License-Identifier: MIT
//! The QA judge on what llama-server's readings add: a reported request's
//! model load and prompt speed in prompt load, llama-server's answering
//! requests, and its lines in Diagnostics, awake and asleep.
use super::*;
use crate::model_facts::ModelFacts;
use crate::providers::llama::LlamaReadings;

const MODEL: &str = "Qwen3-8B-Q4_K_M.gguf";

/// The dashboard fixture with llama-server serving, its facts as
/// `/v1/models` and `/props` give them.
fn llama_app() -> App {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    let sample = &mut app.collector.current;
    sample.runtime.provider = Some("llama.cpp".into());
    sample.runtime.endpoint = "http://127.0.0.1:9931".into();
    sample.runtime.credentials_present = false;
    sample.llm_provider = "llama.cpp".into();
    sample.llm_model = MODEL.into();
    sample.llm_status = "processing".into();
    sample.llm_cache_efficiency = Some(76.9);
    sample.llm_prefix_hit_rate = Some(76.9);
    sample.model = ModelFacts {
        parameters: Some(8_190_735_360),
        bits_per_weight: Some(4.9107),
        quantization: Some("Q4_K - Medium".into()),
        context_configured: Some(32768),
        context_trained: Some(40960),
        draft: Some(true),
        runtime_version: Some("llama.cpp b8240-4f1e2a7".into()),
        loaded_bytes: Some(5_027_783_488),
        ..ModelFacts::default()
    };
    app
}

/// A request a client's recorder reported, with the timing its runtime
/// sent: a 30.7k prompt, 24.6k of it from the cache, read at 812 tok/s after
/// a 3.2-second model load.
fn reported_request(app: &mut App) {
    let request = domain::RequestUsage {
        provider: "llama.cpp".into(),
        model: MODEL.into(),
        id: "chatcmpl-7f3a9c".into(),
        prompt: 30720,
        cached: Some(24576),
        output: Some(512),
        output_tps: Some(40.0),
        completed: true,
        observed_at: Some(SystemTime::now() - Duration::from_secs(20)),
        ..domain::RequestUsage::default()
    };
    app.collector
        .request_history
        .observe(std::slice::from_ref(&request));
    let sample = &mut app.collector.current;
    sample.llm_requests = vec![request.clone()];
    sample.llm_request_timings = vec![domain::RequestTiming {
        provider: request.provider,
        model: request.model,
        id: request.id,
        prefill_tps: Some(812.4),
        load_ms: Some(3250),
    }];
}

/// llama-server answering a request whose prompt it has read, as its
/// `/slots` gives it, with a second slot still reading its prompt.
fn answering(app: &mut App) {
    let request = domain::RequestUsage {
        provider: "llama.cpp".into(),
        model: MODEL.into(),
        id: "4211".into(),
        prompt: 30720,
        cached: Some(24576),
        output: Some(293),
        output_tps: Some(27.0),
        observed_at: Some(SystemTime::now()),
        ..domain::RequestUsage::default()
    };
    app.collector
        .request_history
        .observe(std::slice::from_ref(&request));
    let sample = &mut app.collector.current;
    sample.llm_requests = vec![request];
    sample.llm_active_requests = Some(2);
    sample.llm_generation_tps = Some(27.0);
    sample.llm_generation_tps_live = true;
    sample.llm_kv_free_tokens = Some(1756);
    sample.llama = LlamaReadings {
        slots: Some(4),
        busy_per_decode: Some(1.6),
        reading: Some(12288),
        reading_cached: Some(8192),
        draft_per_pass: Some(3.4),
        draft_acceptance: Some(0.6),
        ..LlamaReadings::default()
    };
}

/// llama-server asleep: idle, its model unloaded.
fn asleep(app: &mut App) {
    let sample = &mut app.collector.current;
    sample.llm_status = "idle".into();
    sample.llm_active_requests = Some(0);
    sample.llm_waiting_requests = Some(0);
    sample.llm_generation_tps_live = false;
    sample.llm_requests.clear();
    sample.llama = LlamaReadings {
        asleep: true,
        slots: Some(4),
        busy_per_decode: Some(1.6),
        ..LlamaReadings::default()
    };
}

/// The screen's words in order, rows run together: a line the panel wraps
/// breaks between words, so a sentence shown whole reads as written.
fn words(app: &App, width: u16, height: u16) -> String {
    text(&draw(app, width, height))
        .join(" ")
        .replace(['│', '┃', '║'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn judged(cases: &[Case]) -> Vec<String> {
    let mut failures = Vec::new();
    for case in cases {
        for (width, height) in SIZES {
            for problem in judge(case, width, height) {
                failures.push(format!("{} at {width}×{height}: {problem}", case.name));
            }
        }
    }
    failures
}

#[test]
fn a_reported_request_shows_its_model_load_and_prompt_speed_whole_or_not_at_all() {
    let mut app = llama_app();
    reported_request(&mut app);
    let case = Case {
        name: "llama-reported-timing",
        impact: false,
        app,
    };
    let mut failures = judged(std::slice::from_ref(&case));
    for (width, height) in SIZES {
        let screen = render_app(&case.app, width, height);
        for (whole, start) in [
            ("MODEL LOAD 3.2s", "MODEL LOAD"),
            ("PREFILL 812 tok/s", "PREFILL 812"),
        ] {
            if screen.contains(start) && !screen.contains(whole) {
                failures.push(format!("{width}×{height}: {start:?} cut\n{screen}"));
            }
        }
        // Where the panel has room, both are there.
        if width >= 160
            && !(screen.contains("MODEL LOAD 3.2s") && screen.contains("PREFILL 812 tok/s"))
        {
            failures.push(format!("{width}×{height}: timings missing\n{screen}"));
        }
    }
    // A warm model's hand-over and a prompt read mostly from the cache
    // are not shown: they say nothing about load or prefill.
    let mut app = case.app;
    let timing = &mut app.collector.current.llm_request_timings[0];
    timing.load_ms = Some(12);
    let entry = app.collector.request_history.entries.back_mut().unwrap();
    entry.usage.cached = Some(30500);
    let screen = render_app(&app, 200, 60);
    assert!(!screen.contains("MODEL LOAD"), "{screen}");
    assert!(!screen.contains("PREFILL 812"), "{screen}");
    // A long load reads in minutes.
    app.collector.current.llm_request_timings[0].load_ms = Some(75_400);
    let screen = render_app(&app, 200, 60);
    assert!(screen.contains("MODEL LOAD 1m 15s"), "{screen}");
    assert!(failures.is_empty(), "QA judge:\n{}", failures.join("\n"));
}

#[test]
fn llama_server_requests_and_diagnostics_pass_the_qa_judge() {
    let mut live = llama_app();
    answering(&mut live);
    let mut diagnostics = llama_app();
    answering(&mut diagnostics);
    diagnostics.diagnostics_open = true;
    let mut sleeping = llama_app();
    asleep(&mut sleeping);
    let mut sleeping_diagnostics = llama_app();
    asleep(&mut sleeping_diagnostics);
    sleeping_diagnostics.diagnostics_open = true;
    let cases = [
        Case {
            name: "llama-answering",
            impact: false,
            app: live,
        },
        Case {
            name: "llama-diagnostics",
            impact: false,
            app: diagnostics,
        },
        Case {
            name: "llama-asleep",
            impact: false,
            app: sleeping,
        },
        Case {
            name: "llama-asleep-diagnostics",
            impact: false,
            app: sleeping_diagnostics,
        },
    ];
    let mut failures = judged(&cases);
    // Each reading once, whole, where the panel shows it: on the first page
    // from 120 columns, and never cut anywhere.
    let readings = [
        (
            "llama-diagnostics",
            vec![
                "Reading a prompt: 12.3k tokens so far, 8.2k of them from the cache",
                "Slots: 4 · 1.6 busy per decode step since the server started",
                "Speculative decoding: 3.4 tokens per verify pass, 60% of drafted tokens accepted, since the server started",
            ],
        ),
        (
            "llama-asleep-diagnostics",
            vec![
                "Asleep: the model is unloaded until the next request loads it again; mlxtop does not wake it.",
                "Slots: 4 · 1.6 busy per decode step since the server started",
            ],
        ),
    ];
    for (name, lines) in readings {
        let case = cases.iter().find(|case| case.name == name).unwrap();
        for (width, height) in SIZES {
            let screen = words(&case.app, width, height);
            for line in &lines {
                let shown = screen.matches(line).count();
                if shown > 1 || (width >= 120 && shown != 1) {
                    failures.push(format!(
                        "{name} at {width}×{height}: {line:?} shown {shown} times"
                    ));
                }
            }
        }
    }
    // Asleep is no error, and says so only in Diagnostics.
    let overview = render_app(&cases[2].app, 200, 60);
    assert!(!overview.contains("Asleep"), "{overview}");
    assert!(!overview.contains("unavailable"), "{overview}");
    assert!(failures.is_empty(), "QA judge:\n{}", failures.join("\n"));
}
