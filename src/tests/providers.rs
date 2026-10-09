use crate::test_support::*;
// SPDX-License-Identifier: MIT
use super::*;

fn record(provider: &str, usage: Value) -> Value {
    json!({"provider": provider, "request_id": "req-1", "model": "test-model",
        "observed_at": 1700000000_u64, "usage": usage})
}

#[test]
fn llama_sums_active_slots_without_inventing_missing_output() {
    let mut slots = json!([
        {"is_processing":true,"next_token":{"n_decoded":12}},
        {"is_processing":true,"next_token":{"n_decoded":8}},
        {"is_processing":false,"next_token":{"n_decoded":999}}
    ]);
    let result = parse_llama_slots(&slots).unwrap();
    assert_eq!(result.active_requests, Some(2));
    assert_eq!(result.output_tokens, Some(20));
    let sample = Sample {
        llm_output_tokens: result.output_tokens,
        ..Sample::default()
    };
    assert_eq!(llm_context_tokens(&sample), None);
    slots[1]["next_token"] = json!({});
    assert_eq!(parse_llama_slots(&slots).unwrap().output_tokens, None);
}

#[test]
fn llama_metrics_work_without_slots_and_keep_average_rates() {
    let metrics = "# TYPE llamacpp:requests_processing gauge\nllamacpp:requests_processing 2\nllamacpp:requests_deferred 3\nllamacpp:predicted_tokens_seconds 24.5\nllamacpp:prompt_tokens_seconds 100\n";
    let result = merge_llama_metrics(None, Some(metrics)).unwrap();
    assert_eq!(result.active_requests, Some(2));
    assert_eq!(result.waiting_requests, Some(3));
    assert_eq!(result.generation_tps, Some(24.5));
    assert_eq!(result.prefill_tps, Some(100.0));
    assert!(!result.generation_tps_live);
    assert!(!result.prefill_tps_live);
    assert_eq!(result.prompt_tokens, None);
    assert!(merge_llama_metrics(None, Some("<html>disabled</html>")).is_none());
    assert_eq!(metric_count("count 1.5", "count"), None);
    assert_eq!(metric_count("count -1", "count"), None);
    assert_eq!(metric_count("count +Inf", "count"), None);
    let idle = parse_llama_slots(&json!([]));
    assert_eq!(
        merge_llama_metrics(idle, None).unwrap().active_requests,
        Some(0)
    );
}

#[test]
fn response_usage_and_incomplete_ollama_chunks() {
    let usage = parse_usage(&record(
        "LocalAI",
        json!({"input_tokens":50,
        "output_tokens":4,"input_tokens_details":{"cached_tokens":20}}),
    ))
    .unwrap();
    assert_eq!(usage.output_tokens, Some(4));
    assert_eq!(usage.requests[0].cached, Some(20));
    let mut streaming = record("Ollama", json!({"prompt_eval_count":50}));
    streaming["done"] = json!(false);
    assert!(parse_usage(&streaming).is_none());
}

#[test]
fn usage_file_supplements_live_slots_and_filters_other_providers() {
    let path = env::temp_dir().join(format!("mlxtop-mixed-{}.jsonl", std::process::id()));
    let llama = record(
        "llama-server",
        json!({"prompt_tokens":400,"completion_tokens":30}),
    );
    let mut ollama = record("ollama", json!({"prompt_eval_count":999}));
    ollama["observed_at"] = json!(1700000001);
    fs::write(&path, format!("{llama}\n{llama}\n{ollama}\n")).unwrap();
    let adapter = Adapter {
        configured: Some("llama.cpp".into()),
        usage_file: Some(path.clone()),
        selected: Some("llama.cpp".into()),
        port: None,
        cached: parse_llama_slots(&json!([{"is_processing":true,"next_token":{"n_decoded":5}}])),
        next_poll: Instant::now(),
        backoff: Duration::from_secs(1),
        kobold_uptime: None,
        kobold_session: 0,
        endpoint: native::Endpoint::default(),
        metrics: native::MetricsHistory::default(),
        diagnostics: Default::default(),
        config_error: None,
    };
    let result = adapter.with_usage().unwrap();
    assert_eq!(result.source, TelemetrySource::Live);
    assert_eq!(result.active_requests, Some(1));
    assert_eq!(result.output_tokens, Some(5));
    assert_eq!(result.prompt_tokens, None);
    assert_eq!(result.requests.len(), 1);
    assert_eq!(result.requests[0].prompt, 400);
    assert!(result.requests[0].completed);
    assert_eq!(read_usage_file(&path, None).unwrap().requests.len(), 1);
    fs::write(&path, "invalid\n").unwrap();
    assert_eq!(adapter.with_usage().unwrap().active_requests, Some(1));
    fs::remove_file(path).unwrap();
}

#[test]
fn llama_http_polls_metrics_even_when_slots_are_disabled() {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        for path in ["/slots", "/metrics"] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut bytes = [0; 2048];
            let n = stream.read(&mut bytes).unwrap();
            assert!(
                String::from_utf8_lossy(&bytes[..n]).starts_with(&format!("GET {path} HTTP/1.1"))
            );
            let (status, body) = if path == "/slots" {
                (403, "{}")
            } else {
                (
                    200,
                    "llamacpp:requests_processing 1\nllamacpp:requests_deferred 2\n",
                )
            };
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    let mut adapter = Adapter {
        configured: None,
        usage_file: None,
        selected: Some("llama.cpp".into()),
        port: Some(port),
        cached: None,
        next_poll: Instant::now(),
        backoff: Duration::from_secs(1),
        kobold_uptime: None,
        kobold_session: 0,
        endpoint: native::Endpoint::default(),
        metrics: native::MetricsHistory::default(),
        diagnostics: Default::default(),
        config_error: None,
    };
    let result = adapter.poll().unwrap();
    assert_eq!(result.waiting_requests, Some(2));
    server.join().unwrap();
}

#[test]
fn first_token_timing_is_explicit_not_derived_from_provider_durations() {
    let mut value = record(
        "Ollama",
        json!({"prompt_tokens": 100, "prompt_eval_duration": 999999}),
    );
    assert_eq!(parse_usage(&value).unwrap().requests[0].ttft_ms, None);
    value["timings"] = json!({"time_to_first_token_ms": 1250});
    assert_eq!(parse_usage(&value).unwrap().requests[0].ttft_ms, Some(1250));
    value["timings"] = json!({"time_to_first_token_ms": -1});
    assert_eq!(parse_usage(&value).unwrap().requests[0].ttft_ms, None);
}

#[test]
fn response_formats_preserve_full_prompt_and_cache_counts() {
    for provider in ["omlx", "mlx_lm.server", "llama.cpp", "koboldcpp", "localai"] {
        let telemetry = parse_usage(&record(
            provider,
            json!({
                "prompt_tokens": 10000, "completion_tokens": 12,
                "prompt_tokens_details": {"cached_tokens": 9000}
            }),
        ))
        .unwrap();
        assert_eq!(telemetry.prompt_tokens, Some(10000));
        assert_eq!(telemetry.requests[0].cached, Some(9000));
        assert_eq!(telemetry.source, TelemetrySource::Report);
        assert!(!telemetry.generation_tps_live);
        assert!(telemetry.cache_efficiency.is_none()); // no aggregate/request mixing
    }
    let ollama = parse_usage(&record(
        "Ollama",
        json!({"prompt_eval_count": 800, "eval_count": 42}),
    ))
    .unwrap();
    assert_eq!(ollama.prompt_tokens, Some(800));
    assert_eq!(ollama.output_tokens, Some(42));
    let lmstudio = parse_usage(
        &json!({"provider":"LM Studio", "request_id":"r", "observed_at":1700000000,
        "stats":{"input_tokens":333,"total_output_tokens":22}}),
    )
    .unwrap();
    assert_eq!(lmstudio.prompt_tokens, Some(333));
}

#[test]
fn missing_invalid_and_future_usage_stays_unavailable() {
    for usage in [
        json!({}),
        json!({"prompt_tokens": -1}),
        json!({"prompt_tokens": 1.5}),
        json!({"prompt_tokens":"123"}),
    ] {
        assert!(parse_usage(&record("Ollama", usage)).is_none());
    }
    let mut value = record("mlx-lm", json!({"prompt_tokens":0,"cached_tokens":1}));
    let parsed = parse_usage(&value).unwrap();
    assert_eq!(parsed.prompt_tokens, Some(0));
    assert_eq!(parsed.requests[0].cached, None);
    value["observed_at"] = json!(u64::MAX);
    assert!(parse_usage(&value).is_none());
    value["observed_at"] = json!(1700000000);
    value.as_object_mut().unwrap().remove("request_id");
    assert!(parse_usage(&value).is_none());
}

#[test]
fn omlx_collects_every_model_and_request_without_aggregate_cache() {
    let requests = omlx_requests(&json!({"cache_efficiency":99,"active_models":{"models":[
        {"id":"a","generating":[{"request_id":"1","prompt_tokens":100},{"request_id":"2","prompt_tokens":200}],
         "waiting":[{"request_id":"3","prompt_tokens":0}]},
        {"id":"b","prefilling":[{"request_id":"4","prompt_tokens":300,"cached_tokens":100}]}
    ]}}));
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1].prompt, 200);
    assert_eq!(requests[0].cached, None);
    assert_eq!(requests[2].cached, Some(100));
}

#[test]
fn output_speeds_belong_to_individual_generating_requests() {
    let requests = omlx_requests(&json!({"avg_generation_tps":99,"active_models":{"models":[
        {"id":"a","generating":[
            {"request_id":"1","prompt_tokens":100,"tokens_per_second":24.5},
            {"request_id":"2","prompt_tokens":200,"tokens_per_second":38.0},
            {"request_id":"3","prompt_tokens":300}],
         "prefilling":[{"request_id":"4","prompt_tokens":400,"speed":800.0}],
         "waiting":[{"request_id":"5","prompt_tokens":500,"tokens_per_second":100.0}]},
        {"id":"b","generating":[{"request_id":"1","prompt_tokens":100,"tokens_per_second":12.0}]}
    ]}}));
    assert_eq!(
        requests
            .iter()
            .map(|request| request.output_tps)
            .collect::<Vec<_>>(),
        [Some(24.5), Some(38.0), None, None, None, Some(12.0)]
    );
    let distributed = omlx_requests(&json!({"active_models":{"models":[
        omlx_cluster_model(json!([
            cluster_request("a", 100, 30, false),
            cluster_request("b", 200, 0, true)
        ]), 0.5, false)
    ]}}));
    assert_eq!(distributed[0].output_tps, Some(12.0));
    assert_eq!(distributed[1].output_tps, None);
}

#[test]
fn completed_request_speeds_use_decode_timing_and_reject_missing_or_invalid_data() {
    let mut value = record(
        "Ollama",
        json!({"prompt_eval_count":100,"eval_count":80,
        "eval_duration":2_000_000_000_u64,"prompt_eval_duration":1_000_000_000_u64,
        "total_duration":10_000_000_000_u64}),
    );
    let request = parse_usage(&value).unwrap().requests.remove(0);
    assert_eq!(request.output_tps, Some(40.0));
    assert!(request.completed);
    for duration in [json!(0), json!(-1), json!(null), json!("2 seconds")] {
        value["usage"]["eval_duration"] = duration;
        assert_eq!(parse_usage(&value).unwrap().requests[0].output_tps, None);
    }
    value["usage"]["eval_duration"] = json!(1_000_000_000_u64);
    value["usage"]["eval_count"] = json!(0);
    assert_eq!(
        parse_usage(&value).unwrap().requests[0].output_tps,
        Some(0.0)
    );
    let mut value = record(
        "oMLX",
        json!({"prompt_tokens":100,"completion_tokens":80,"avg_generation_tps":99}),
    );
    assert_eq!(parse_usage(&value).unwrap().requests[0].output_tps, None);
    value["timings"] = json!({"output_tokens_per_second":32.5});
    assert_eq!(
        parse_usage(&value).unwrap().requests[0].output_tps,
        Some(32.5)
    );
    value["timings"] = json!({"output_tokens_per_second":-1});
    assert_eq!(parse_usage(&value).unwrap().requests[0].output_tps, None);
}

/// An oMLX 0.7 distributed model as `_build_active_models_data` renders it:
/// one synthetic `rank0` row beside rank zero's per-request metrics.
fn omlx_cluster_model(running: Value, age_seconds: f64, stale: bool) -> Value {
    let last = running.as_array().and_then(|r| r.last()).cloned();
    json!({"id":"dist","active_requests":running.as_array().map_or(0, Vec::len),
        "waiting_requests":0,"prefilling":[],
        "generating":[{"request_id":"rank0","generated_tokens":9,"prompt_tokens":500,
            "tokens_per_second":12.0}],
        "cluster":{"deployment_id":"d1","live":{"age_seconds":age_seconds,"stale":stale,
            "metrics":{"scope":"end_to_end_pipeline","active_requests":2,
                "aggregate_decode_tps":0.4,"active_request_metrics":running,
                "active_request_metrics_truncated":0,"last_request":last}}}})
}

fn cluster_request(id: &str, prompt: u64, completion: u64, prefilling: bool) -> Value {
    json!({"status":"running","request_id":id,"prompt_tokens":prompt,"cached_tokens":100,
        "completion_tokens":completion,"decode_tps":if prefilling { 0.0 } else { 12.0 },
        "prefill_progress":{"active":prefilling,"processed":64,"total":prompt,"speed":80.0}})
}

#[test]
fn omlx_cluster_rows_use_rank_zero_request_ids() {
    let model = omlx_cluster_model(
        json!([
            cluster_request("chatcmpl-a", 500, 9, false),
            cluster_request("chatcmpl-b", 700, 0, true)
        ]),
        2.0,
        false,
    );
    let rows = omlx_model_requests(&model);
    assert_eq!(rows.len(), 2, "the synthetic row is replaced, not added");
    assert_eq!(rows[0].id.as_deref(), Some("chatcmpl-a"));
    assert_eq!(rows[0].phase, OmlxPhase::Generating);
    assert_eq!(rows[0].rate, Some(12.0));
    assert_eq!(rows[1].phase, OmlxPhase::Prefilling);
    assert_eq!(rows[1].rate, Some(80.0));
    assert_eq!(rows[1].age, Some(Duration::from_secs(2)));

    let before = SystemTime::now();
    let requests = omlx_requests(&json!({"active_models":{"models":[model]}}));
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| request.id != "rank0"));
    assert_eq!(requests[1].cached, Some(100));
    // Rank zero's marker age, not the poll time, dates the observation.
    assert!(requests[0].observed_at.unwrap() <= before - Duration::from_secs(1));
}

#[test]
fn successive_omlx_cluster_requests_stay_distinct() {
    let mut history = request_history::History::default();
    for id in ["chatcmpl-1", "chatcmpl-2"] {
        let model = omlx_cluster_model(json!([cluster_request(id, 500, 9, false)]), 0.5, false);
        history.observe(&omlx_requests(&json!({"active_models":{"models":[model]}})));
    }
    assert_eq!(history.len(), 2);
}

#[test]
fn omlx_cluster_placeholder_falls_back_to_last_running_request() {
    let mut model = omlx_cluster_model(
        json!([cluster_request("chatcmpl-a", 500, 9, false)]),
        0.5,
        false,
    );
    let metrics = &mut model["cluster"]["live"]["metrics"];
    metrics
        .as_object_mut()
        .unwrap()
        .remove("active_request_metrics");
    let rows = omlx_model_requests(&model);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id.as_deref(), Some("chatcmpl-a"));
    assert_eq!(rows[0].rate, Some(12.0));

    // Without any identity the row still counts as work, not as history.
    model["cluster"]["live"]["metrics"]["last_request"]["status"] = json!("completed");
    assert_eq!(omlx_model_requests(&model)[0].id, None);
    assert!(omlx_requests(&json!({"active_models":{"models":[model]}})).is_empty());
}

#[test]
fn stale_omlx_cluster_metrics_are_ignored() {
    let model = omlx_cluster_model(
        json!([cluster_request("chatcmpl-a", 500, 9, false)]),
        30.0,
        true,
    );
    let rows = omlx_model_requests(&model);
    // Stale metrics add no requests and give the placeholder no identity.
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, None);
    assert_eq!(rows[0].age, None);
}

#[test]
fn journal_deduplicates_polling_but_preserves_equal_sized_requests() {
    let mut seen = VecDeque::new();
    let mut request = parse_usage(&record("Ollama", json!({"prompt_tokens":10})))
        .unwrap()
        .requests
        .remove(0);
    assert!(new_request_summary(&mut seen, &request).is_some());
    assert!(new_request_summary(&mut seen, &request).is_none());
    request.id = "req-2".into();
    assert!(new_request_summary(&mut seen, &request).is_some());
    for n in 0..600 {
        request.id = n.to_string();
        new_request_summary(&mut seen, &request);
    }
    assert_eq!(seen.len(), 512);
}

#[test]
fn kobold_last_result_never_becomes_a_live_rate() {
    let result = parse_kobold(
        &json!({"total_gens":2,"last_input_count":1000,"last_token_count":20,
        "last_eval_speed":30,"last_process_speed":500,"idle":0}),
    )
    .unwrap();
    assert_eq!(result.prompt_tokens, Some(1000));
    assert_eq!(result.source, TelemetrySource::Report);
    assert_eq!(result.status.as_deref(), Some("last result"));
    assert!(!result.generation_tps_live);
    assert_eq!(
        parse_kobold(&json!({"total_gens":0,"last_input_count":0}))
            .unwrap()
            .prompt_tokens,
        None
    );
    assert!(parse_kobold(&json!({"status":"ok"})).is_none());
}

#[test]
fn kobold_restart_does_not_reuse_request_identity() {
    let mut adapter = Adapter {
        configured: None,
        usage_file: None,
        selected: Some("KoboldCpp".into()),
        port: None,
        cached: None,
        next_poll: Instant::now(),
        backoff: Duration::from_secs(1),
        kobold_uptime: None,
        kobold_session: 0,
        endpoint: native::Endpoint::default(),
        metrics: native::MetricsHistory::default(),
        diagnostics: Default::default(),
        config_error: None,
    };
    let first = adapter
        .kobold_result(&json!({"total_gens":1,"last_input_count":100,"uptime":500.5}))
        .unwrap();
    let repeated = adapter
        .kobold_result(&json!({"total_gens":1,"last_input_count":100,"uptime":501.5}))
        .unwrap();
    let restarted = adapter
        .kobold_result(&json!({"total_gens":1,"last_input_count":200,"uptime":5.0}))
        .unwrap();
    assert_eq!(first.requests[0].id, repeated.requests[0].id);
    assert_ne!(first.requests[0].id, restarted.requests[0].id);
    assert_eq!(restarted.source, TelemetrySource::Report);
}

#[test]
fn llama_capacity_and_processed_work_are_not_prompt_length() {
    let slots = json!([{"id":0,"id_task":10,"is_processing":true,"n_ctx":65536,
        "timings":{"prompt_n":50},"next_token":{"n_decoded":12}},
        {"id":1,"is_processing":false,"next_token":{"n_decoded":999}}]);
    let result = parse_llama_slots(&slots).unwrap();
    assert_eq!(result.active_requests, Some(1));
    assert_eq!(result.output_tokens, Some(12));
    assert_eq!(result.prompt_tokens, None);
    assert!(parse_llama_slots(&json!([{}])).is_none());
    let idle = parse_llama_slots(&json!([])).unwrap();
    assert_eq!(idle.active_requests, Some(0));
    assert_eq!(idle.output_tokens, None);
    assert_eq!(
        metric(
            "llamacpp:predicted_tokens_seconds 25\n",
            "llamacpp:predicted_tokens_seconds"
        ),
        Some(25.0)
    );
    assert_eq!(metric("rate NaN\n", "rate"), None);
}

#[test]
fn usage_file_ignores_partial_and_bad_records_and_keeps_original_time() {
    let path = env::temp_dir().join(format!(
        "mlxtop-usage-test-{}-{}.jsonl",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let valid = record("mlx-lm", json!({"prompt_tokens":1024}));
    fs::write(&path, format!("not json\n{valid}\n{{\"partial\":")).unwrap();
    let result = read_usage_file(&path, None).unwrap();
    assert_eq!(result.prompt_tokens, Some(1024));
    assert_eq!(result.requests.len(), 1);
    assert_eq!(
        result.observed_at,
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1700000000))
    );
    fs::write(&path, valid.to_string()).unwrap();
    assert!(read_usage_file(&path, None).is_none());
    fs::remove_file(path).unwrap();
}

#[test]
fn provider_switch_drops_cached_counts() {
    let mut adapter = Adapter {
        configured: None,
        usage_file: None,
        selected: Some("KoboldCpp".into()),
        port: None,
        cached: parse_kobold(&json!({"total_gens":1,"last_input_count":20})),
        next_poll: Instant::now(),
        backoff: Duration::from_secs(30),
        kobold_uptime: None,
        kobold_session: 0,
        endpoint: native::Endpoint::default(),
        metrics: native::MetricsHistory::default(),
        diagnostics: Default::default(),
        config_error: None,
    };
    assert!(adapter.selected(Some("Ollama")));
    assert!(adapter.cached.is_none());
    assert!(adapter.poll().is_none());
    assert!(!adapter.selected(Some("oMLX")));
}

#[test]
fn native_poll_uses_get_and_preserves_last_result_age() {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut bytes = [0; 2048];
            let n = stream.read(&mut bytes).unwrap();
            assert!(
                String::from_utf8_lossy(&bytes[..n]).starts_with("GET /api/extra/perf HTTP/1.1")
            );
            let body = r#"{"total_gens":1,"last_input_count":456}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        }
    });
    let mut adapter = Adapter {
        configured: Some("KoboldCpp".into()),
        usage_file: None,
        selected: Some("KoboldCpp".into()),
        port: Some(port),
        cached: None,
        next_poll: Instant::now(),
        backoff: Duration::from_secs(1),
        kobold_uptime: None,
        kobold_session: 0,
        endpoint: native::Endpoint::default(),
        metrics: native::MetricsHistory::default(),
        diagnostics: Default::default(),
        config_error: None,
    };
    let first = adapter.poll().unwrap();
    adapter.next_poll = Instant::now();
    let second = adapter.poll().unwrap();
    assert_eq!(first.prompt_tokens, Some(456));
    assert_eq!(first.observed_at, second.observed_at);
    server.join().unwrap();
}

#[test]
fn request_summaries_label_sampled_and_final_output_speeds() {
    let mut request = RequestUsage {
        provider: "oMLX".into(),
        model: "qwen".into(),
        id: "r1".into(),
        prompt: 800,
        cached: None,
        output: Some(64),
        output_tps: Some(41.25),
        completed: false,
        ttft_ms: None,
        observed_at: None,
    };
    assert_eq!(
        request.summary(),
        "observed prompt 800 · out 64 · oMLX · qwen · r1 · output 41.2 tok/s (sampled)"
    );
    request.completed = true;
    request.cached = Some(200);
    request.ttft_ms = Some(310);
    request.output = None;
    assert_eq!(
        request.summary(),
        "reported prompt 800 · out — · oMLX · qwen · r1 · cached 200 · output 41.2 tok/s (request avg) · first token 310 ms (reported)"
    );
}

#[test]
fn identifiers_accept_numbers_and_reject_structures_or_empty_text() {
    assert_eq!(identifier(&json!({"id": 42}), "id").as_deref(), Some("42"));
    assert_eq!(
        identifier(&json!({"id": "a\u{0007}b"}), "id").as_deref(),
        Some("ab")
    );
    assert_eq!(identifier(&json!({"id": {"nested": 1}}), "id"), None);
    assert_eq!(identifier(&json!({"id": "\u{0007}"}), "id"), None);
    assert_eq!(identifier(&json!({}), "id"), None);
    assert_eq!(canonical_provider("vLLM"), Some("vLLM"));
}

#[test]
fn llama_metrics_status_follows_processing_and_deferred_counters() {
    let status = |metrics: &str| {
        merge_llama_metrics(None, Some(metrics))
            .unwrap()
            .status
            .unwrap()
    };
    assert_eq!(status("llamacpp:requests_processing 0\n"), "idle");
    assert_eq!(
        status("llamacpp:requests_processing 0\nllamacpp:requests_deferred 1\n"),
        "processing"
    );
    assert_eq!(status("llamacpp:requests_processing 2\n"), "processing");
    assert_eq!(
        status("llamacpp:predicted_tokens_seconds 12.5\n"),
        "running"
    );
    assert_eq!(
        metric_count(
            "llamacpp:requests_processing 1.5\n",
            "llamacpp:requests_processing"
        ),
        None
    );
}

#[test]
fn usage_files_skip_a_partial_head_and_reject_non_files() {
    let dir = env::temp_dir().join(format!("mlxtop-usage-tail-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    assert!(
        read_usage_file(&dir, None).is_none(),
        "directories are not usage files"
    );
    assert!(read_usage_file(&dir.join("missing.jsonl"), None).is_none());

    // A file larger than the tail window starts mid-record; that record is
    // skipped and the complete ones after it are kept.
    let path = dir.join("usage.jsonl");
    let filler = "x".repeat(MAX_USAGE_BYTES as usize);
    let valid = record("mlx-lm", json!({"prompt_tokens": 77}));
    fs::write(&path, format!("{filler}\n{valid}\n")).unwrap();
    let result = read_usage_file(&path, None).unwrap();
    assert_eq!(result.prompt_tokens, Some(77));
    // A window that holds only the tail of one huge line has no record.
    fs::write(
        &path,
        format!("{}\n", "y".repeat(2 * MAX_USAGE_BYTES as usize)),
    )
    .unwrap();
    assert!(read_usage_file(&path, None).is_none());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn future_usage_records_are_ignored() {
    let future = json!({"provider": "mlx-lm", "request_id": "r", "observed_at": u64::MAX / 2,
        "usage": {"prompt_tokens": 1}});
    assert!(parse_usage(&future).is_none());
    let unfinished = json!({"done": false, "provider": "ollama"});
    assert!(parse_usage(&unfinished).is_none());
}

#[test]
fn every_new_adapter_polls_its_native_read_only_endpoint() {
    for (provider, paths) in [
        (
            "Ollama",
            vec![("/api/ps", r#"{"models":[{"name":"test","size_vram":1}]}"#)],
        ),
        (
            "LM Studio",
            vec![(
                "/api/v1/models",
                r#"{"models":[{"loaded_instances":[{"id":"test"}]}]}"#,
            )],
        ),
        (
            "LM Studio",
            vec![
                ("/api/v1/models", "{}"),
                (
                    "/api/v0/models",
                    r#"{"data":[{"id":"test","state":"loaded"}]}"#,
                ),
            ],
        ),
        (
            "LM Studio",
            vec![
                ("/api/v1/models", "{}"),
                ("/api/v0/models", "{}"),
                ("/v1/models", r#"{"data":[{"id":"test"}]}"#),
            ],
        ),
        (
            "mlx-lm",
            vec![("/v1/models", r#"{"data":[{"id":"test"}]}"#)],
        ),
        (
            "LocalAI",
            vec![("/v1/models", r#"{"data":[{"id":"test"}]}"#)],
        ),
        ("Jan", vec![("/v1/models", r#"{"data":[{"id":"test"}]}"#)]),
        (
            "GPT4All",
            vec![("/v1/models", r#"{"data":[{"id":"test"}]}"#)],
        ),
        ("vLLM", vec![("/metrics", "vllm:num_requests_running 1\n")]),
        ("SGLang", vec![("/metrics", "sglang:num_running_reqs 1\n")]),
    ] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            for (path, body) in paths {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut data = [0; 4096];
                let n = stream.read(&mut data).unwrap();
                assert!(String::from_utf8_lossy(&data[..n])
                    .starts_with(&format!("GET {path} HTTP/1.1")));
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        let mut adapter = Adapter {
            configured: Some(provider.into()),
            usage_file: None,
            selected: None,
            port: Some(port),
            cached: None,
            next_poll: Instant::now(),
            backoff: Duration::from_secs(1),
            kobold_uptime: None,
            kobold_session: 0,
            endpoint: native::Endpoint::default(),
            metrics: native::MetricsHistory::default(),
            diagnostics: Default::default(),
            config_error: None,
        };
        assert!(adapter.selected(Some("oMLX")));
        let result = adapter.poll().unwrap();
        assert_eq!(result.provider.as_deref(), Some(provider));
        assert_eq!(result.source, TelemetrySource::Live);
        let observed = result.observed_at;
        assert_eq!(
            adapter.poll().unwrap().observed_at,
            observed,
            "cached polls retain age"
        );
        server.join().unwrap();
        adapter.next_poll = Instant::now();
        assert_eq!(
            adapter.poll().unwrap().observed_at,
            observed,
            "connection failure retains age"
        );
        assert_eq!(adapter.backoff, Duration::from_secs(2));
    }
}

#[test]
fn lm_native_usage_preserves_decode_and_explicit_first_token_timing() {
    let mut record = record(
        "lmstudio",
        json!({"input_tokens":40,"total_output_tokens":10,
        "tokens_per_second":25.0,"time_to_first_token_seconds":0.25}),
    );
    let result = parse_usage(&record).unwrap();
    assert_eq!(result.requests[0].output_tps, Some(25.0));
    assert_eq!(result.requests[0].ttft_ms, Some(250));
    assert!(!result.generation_tps_live);
    record["usage"]["time_to_first_token_seconds"] = json!(-1);
    assert_eq!(parse_usage(&record).unwrap().requests[0].ttft_ms, None);
}

fn diagnostic_adapter(provider: &str, port: u16) -> Adapter {
    Adapter {
        configured: Some(provider.into()),
        selected: None,
        usage_file: None,
        port: Some(port),
        cached: None,
        next_poll: Instant::now(),
        backoff: Duration::from_secs(1),
        kobold_uptime: None,
        kobold_session: 0,
        endpoint: native::Endpoint::default(),
        metrics: native::MetricsHistory::default(),
        diagnostics: Default::default(),
        config_error: None,
    }
}

#[test]
fn runtime_report_distinguishes_auth_schema_and_optional_endpoint_failures() {
    use crate::runtime_diagnostics::ProbeIssue;
    for (status, body, issue) in [
        (401, "{}", ProbeIssue::Http(401)),
        (200, "not json", ProbeIssue::InvalidResponse),
        (200, "{}", ProbeIssue::InvalidResponse),
    ] {
        let (port, server) = serve(vec![reply("GET /api/ps", status, body)]);
        let mut adapter = diagnostic_adapter("ollama", port);
        assert!(adapter.selected(None));
        assert!(adapter.poll().is_none());
        let report = adapter.report();
        assert!(report.failed());
        assert!(report.probes.iter().any(|p| p.issue == Some(issue.clone())));
        server.join().unwrap();
    }
    let (port, server) = serve(vec![
        reply("GET /slots", 404, "{}"),
        reply("GET /metrics", 200, "llamacpp:requests_processing 0\n"),
    ]);
    let mut adapter = diagnostic_adapter("llama.cpp", port);
    adapter.selected(None);
    assert!(adapter.poll().is_some());
    let report = adapter.report();
    assert!(report.connected && !report.failed());
    assert!(report.status().contains("partial"));
    server.join().unwrap();
}

#[test]
fn legacy_fallback_is_connected_and_invalid_selection_never_polls() {
    use crate::runtime_diagnostics::ProbeIssue;
    let (port, server) = serve(vec![
        reply("GET /api/v1/models", 404, "{}"),
        reply("GET /api/v0/models", 200, r#"{"data":[]}"#),
    ]);
    let mut adapter = diagnostic_adapter("lmstudio", port);
    adapter.selected(None);
    assert!(adapter.poll().is_some());
    assert!(adapter.report().connected);
    assert!(adapter
        .report()
        .probes
        .iter()
        .any(|p| p.path == "/api/v0/models" && p.issue.is_none()));
    server.join().unwrap();
    let mut adapter = diagnostic_adapter("unsupported", 1);
    adapter.selected(None);
    assert!(adapter.poll().is_none());
    assert_eq!(
        adapter.report().probes[0].issue,
        Some(ProbeIssue::InvalidProvider)
    );
    let mut adapter = diagnostic_adapter("ollama", 1);
    adapter.selected(None);
    adapter.config_error = Some(ProbeIssue::InvalidPort);
    assert!(adapter.poll().is_none());
    assert_eq!(
        adapter.report().probes[0].issue,
        Some(ProbeIssue::InvalidPort)
    );
}

#[test]
fn recorder_only_reports_usage_without_claiming_an_api_connection() {
    let dir = TempDir::new("recorder-only-diagnostics");
    let usage = record("ollama", json!({"prompt_eval_count":50}));
    let path = dir.write("usage.jsonl", &format!("{usage}\n"));
    let mut adapter = diagnostic_adapter("ollama", 1);
    adapter.configured = None;
    adapter.usage_file = Some(path.clone());
    assert!(adapter.selected(None));
    assert!(adapter.poll().is_some());
    let report = adapter.report();
    assert_eq!(report.provider.as_deref(), Some("Ollama"));
    assert_eq!(report.status(), "Usage recorder only");
    assert!(!report.failed() && !report.connected);
    assert!(report.attempted_at.is_none() && report.endpoint.is_empty());
    assert!(report.probes.is_empty());
    fs::write(path, "invalid\n").unwrap();
    assert!(adapter.poll().is_none());
    assert!(adapter.report().usage.contains("no valid"));
    assert!(!adapter.report().failed());
}

#[test]
fn mlx_serve_stops_asking_for_metrics_json_after_a_404_until_restart() {
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let paths = Arc::new(Mutex::new(Vec::<String>::new()));
    let start = Arc::new(Mutex::new(1_700_000_000_u64));
    let (log, boot) = (paths.clone(), start.clone());
    let server = thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut bytes = [0; 2048];
            let n = stream.read(&mut bytes).unwrap_or(0);
            let request = String::from_utf8_lossy(&bytes[..n]).to_string();
            let Some(path) = request.split_whitespace().nth(1).map(str::to_owned) else {
                break;
            };
            log.lock().unwrap().push(path.clone());
            let (status, body) = if path == "/metrics" {
                ("200 OK", format!("vllm:num_requests_running 1\nvllm:num_requests_waiting 0\nprocess_start_time_seconds {}\n", boot.lock().unwrap()))
            } else {
                ("404 Not Found", String::new())
            };
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    let mut adapter = Adapter {
        configured: Some("mlx-serve".into()),
        usage_file: None,
        selected: Some("mlx-serve".into()),
        port: Some(port),
        cached: None,
        next_poll: Instant::now(),
        backoff: Duration::from_secs(1),
        kobold_uptime: None,
        kobold_session: 0,
        endpoint: native::Endpoint::default(),
        metrics: native::MetricsHistory::default(),
        diagnostics: Default::default(),
        config_error: None,
    };
    for _ in 0..3 {
        adapter.next_poll = Instant::now();
        assert!(adapter.poll().is_some());
    }
    *start.lock().unwrap() += 60; // the server restarted: it may have been upgraded
    adapter.next_poll = Instant::now();
    assert!(adapter.poll().is_some());
    drop(std::net::TcpStream::connect(("127.0.0.1", port)));
    server.join().unwrap();
    let json_requests = paths
        .lock()
        .unwrap()
        .iter()
        .filter(|p| *p == "/metrics.json")
        .count();
    assert_eq!(
        json_requests,
        2,
        "one probe, then one more after the restart: {:?}",
        paths.lock().unwrap()
    );
}
