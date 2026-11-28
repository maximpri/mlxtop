use crate::test_support::*;
// SPDX-License-Identifier: MIT
use super::*;

#[test]
fn inventories_distinguish_loaded_models_from_catalogues_and_capacity() {
    let result =
        ollama(&json!({"models":[{"name":"test", "size_vram":1024, "context_length":8192}]}))
            .unwrap();
    assert_eq!(result.model.as_deref(), Some("test"));
    assert_eq!(result.model_memory, Some(1024));
    // The context it allows is a model fact, said once in Diagnostics.
    assert_eq!(result.model_facts.context_configured, Some(8192));
    assert!(!result.details.unwrap().contains("8192"));
    assert_eq!(result.active_requests, None);
    assert_eq!(result.prompt_tokens, None);
    assert_eq!(result.generation_tps, None);
    assert!(ollama(&json!({"models":[{}]})).is_none());
    assert!(ollama(&json!({})).is_none());
    assert_eq!(
        ollama(&json!({"models":[]})).unwrap().status.as_deref(),
        Some("no models")
    );
    let result = ollama(&json!({"models":[{"name":"b", "size_vram":1},{"model":"a"}]})).unwrap();
    assert_eq!(
        result.model_memory, None,
        "missing allocation invalidates aggregate"
    );
    assert_eq!(result.model.as_deref(), Some("2 models · a"));
    assert_eq!(result.model_offloaded, None);
    // Placement: `size` minus `size_vram` is the part running on CPU.
    let split = ollama(&json!({"models":[
        {"name":"a", "size":10000, "size_vram":6000},
        {"name":"b", "size":2000, "size_vram":2000}
    ]}))
    .unwrap();
    assert_eq!(split.model_size, Some(12000));
    assert_eq!(split.model_offloaded, Some(4000));
    assert!(split.details.unwrap().contains("on CPU"));
    let resident = ollama(&json!({"models":[{"name":"a", "size":10, "size_vram":10}]})).unwrap();
    assert_eq!(resident.model_offloaded, Some(0));
    assert!(!resident.details.unwrap().contains("on CPU"));
    let no_size = ollama(&json!({"models":[{"name":"a", "size_vram":10}]})).unwrap();
    assert_eq!(no_size.model_offloaded, None);
    let lm = lm_studio(&json!({"models":[
        {"key":"downloaded", "loaded_instances":[], "size_bytes":99999},
        {"key":"loaded", "loaded_instances":[{"id":"instance", "config":{"context_length":4096}}]},
        {"type":"embedding", "loaded_instances":[{"id":"embedding"}]},
        {"type":"decision", "loaded_instances":[{"id":"decision"}]},
        {"key":"listed without instances"}
    ]}))
    .unwrap();
    assert_eq!(lm.model.as_deref(), Some("instance"));
    assert_eq!(lm.model_memory, None);
    assert_eq!(lm.model_facts.context_configured, Some(4096));
    assert_eq!(lm.details.as_deref(), Some("1 loaded models"));
    // A model without `loaded_instances` is not loaded, not a broken reply.
    let none = lm_studio(&json!({"models":[{}]})).unwrap();
    assert_eq!(none.model.as_deref(), Some("none"));
    assert!(lm_studio(&json!({"models":[{"loaded_instances":[{}]}]})).is_none());
    let legacy = lm_studio_legacy(
        &json!({"data":[{"id":"loaded", "state":"loaded"}, {"state":"not-loaded"}]}),
    )
    .unwrap();
    assert_eq!(legacy.model.as_deref(), Some("loaded"));
    assert!(lm_studio_legacy(&json!({"data":[{}]})).is_none());
    for provider in ["Jan", "GPT4All", "mlx-lm", "LocalAI", "Magnitude"] {
        let result = models(provider, &json!({"data":[{"id":"available"}]})).unwrap();
        assert_eq!(result.status.as_deref(), Some("available"));
        assert_eq!(result.active_requests, None);
        assert_eq!(result.provider.as_deref(), Some(provider));
    }
    assert!(models("Jan", &json!({"data":[{}]})).is_none());
}

fn vllm(generated: u64, prompt: u64) -> String {
    format!("# HELP ignored\nvllm:num_requests_running{{model_name=\"test model\",engine=\"0\"}} 1\nvllm:num_requests_waiting 2\nvllm:generation_tokens_total{{model_name=\"test model\"}} {generated}\nvllm:prompt_tokens_total{{model_name=\"test model\"}} {prompt}\nvllm:kv_cache_usage_perc 0.25\nvllm:prefix_cache_queries_total 100\nvllm:prefix_cache_hits_total 60\nvllm:time_to_first_token_seconds_sum 4\nvllm:time_to_first_token_seconds_count 2\n")
}

/// `prefill` is the finished-prefill total, which only moves when a request ends;
/// `in_flight` is the live gauge for the prefill running now.
fn mlx_serve(live: u64, prefill: u64, cached: u64, in_flight: u64) -> String {
    format!("# HELP ignored\nvllm:num_requests_running 2\nvllm:num_requests_waiting 1\nvllm:generation_tokens_total 7\nvllm:prompt_tokens_total {}\nmlx_serve:generation_tokens_live {live}\nmlx_serve:prefill_tokens_total {prefill}\nmlx_serve:prefill_tokens_live {in_flight}\nmlx_serve:prefill_tokens_expected 1000\nmlx_serve:requests_prefilling {}\nmlx_serve:prefix_cache_tokens_total {cached}\nmlx_serve:mlx_active_bytes 4096\n", prefill + cached, u8::from(in_flight > 0))
}

/// Only the prefill series: the live gauge, whether a request is prefilling, and the finished-request counters.
fn mlx_serve_prefill(live: u64, prefilling: bool, tokens: u64, seconds: f64) -> String {
    format!("vllm:num_requests_running 1\nmlx_serve:generation_tokens_live 0\nmlx_serve:prefill_tokens_live {live}\nmlx_serve:requests_prefilling {}\nmlx_serve:prefill_tokens_total {tokens}\nvllm:request_prefill_time_seconds_sum {seconds}\n", u8::from(prefilling))
}

/// sushi publishes the same vLLM-shaped queues and its own gauges under `sushi:`.
fn sushi_metrics(text: &str) -> String {
    text.replace("mlx_serve:", "sushi:")
}

#[test]
fn mlx_serve_rates_come_from_the_live_series() {
    let now = Instant::now();
    let mut history = MetricsHistory::default();
    let first = history
        .observe("mlx-serve", &mlx_serve(1000, 400, 1600, 100), now)
        .unwrap();
    assert_eq!(first.provider.as_deref(), Some("mlx-serve"));
    assert_eq!(
        (first.active_requests, first.waiting_requests),
        (Some(2), Some(1))
    );
    assert_eq!((first.generation_tps, first.prefill_tps), (None, None));
    assert_eq!(first.prefix_hit_rate, Some(80.0));
    assert_eq!(first.model_memory, Some(4096));
    assert!(first
        .details
        .as_deref()
        .unwrap()
        .contains("prefill 100 / 1000 tokens (10%)"));
    assert_eq!(
        (first.prefill_tokens_done, first.prefill_tokens_expected),
        (Some(100), Some(1000)),
        "the panel needs the numbers, not only the sentence"
    );
    let second = history
        .observe(
            "mlx-serve",
            &mlx_serve(1300, 400, 1600, 300),
            now + Duration::from_secs(2),
        )
        .unwrap();
    assert_eq!(second.generation_tps, Some(150.0));
    assert_eq!(second.prefill_tps, Some(100.0));
    assert!(second.generation_tps_live && second.prefill_tps_live);
    let restart = history
        .observe(
            "mlx-serve",
            &mlx_serve(5, 5, 5, 5),
            now + Duration::from_secs(3),
        )
        .unwrap();
    assert_eq!(restart.generation_tps, None);
    assert_eq!(restart.prefill_tps, None, "a restart clears the window");
}

#[test]
fn mlx_serve_reads_sushi_gauge_names_too() {
    let now = Instant::now();
    let mut history = MetricsHistory::default();
    let first = history
        .observe(
            "mlx-serve",
            &sushi_metrics(&mlx_serve(1000, 400, 1600, 100)),
            now,
        )
        .unwrap();
    assert_eq!(first.provider.as_deref(), Some("mlx-serve"));
    assert_eq!(
        (first.active_requests, first.waiting_requests),
        (Some(2), Some(1))
    );
    assert_eq!((first.generation_tps, first.prefill_tps), (None, None));
    assert_eq!(first.prefix_hit_rate, Some(80.0));
    assert_eq!(first.model_memory, Some(4096));
    assert!(first
        .details
        .as_deref()
        .unwrap()
        .contains("prefill 100 / 1000 tokens (10%)"));
    let second = history
        .observe(
            "mlx-serve",
            &sushi_metrics(&mlx_serve(1300, 400, 1600, 300)),
            now + Duration::from_secs(2),
        )
        .unwrap();
    assert_eq!(second.generation_tps, Some(150.0));
    assert_eq!(second.prefill_tps, Some(100.0));
    assert!(second.generation_tps_live && second.prefill_tps_live);
    let mut sessions = LlmTelemetry::default();
    apply_mlx_serve_sessions(
        &mut sessions,
        &json!({"sessions":[{"model":"DeepSeek Sushi", "phase":"decode",
            "context_tokens":50, "generated_tokens":20, "request_id":4}]}),
        SystemTime::now(),
        Instant::now(),
        &mut SessionSpeeds::default(),
    );
    assert_eq!(sessions.model.as_deref(), Some("DeepSeek Sushi"));
    assert_eq!(sessions.requests[0].provider, "mlx-serve");
}

#[test]
fn sushi_prefill_reads_its_average_speed_not_the_chunk_size() {
    let now = Instant::now();
    let mut history = MetricsHistory::default();
    let mut live = 0;
    let mut rate = None;
    for second in 0..12_u64 {
        // The gauge gains one 8192-token chunk every other second, like the server's.
        if second % 2 == 1 {
            live += 8192;
        }
        let text = sushi_metrics(&mlx_serve_prefill(live, true, 100, 1.0));
        rate = history
            .observe("mlx-serve", &text, now + Duration::from_secs(second))
            .unwrap()
            .prefill_tps;
    }
    let rate = rate.unwrap();
    assert!(
        (3500.0..4700.0).contains(&rate),
        "{rate}: one second would read 0, 8192 or 16384"
    );
}

#[test]
fn a_long_prefill_reads_its_average_speed_not_the_chunk_size() {
    let now = Instant::now();
    let mut history = MetricsHistory::default();
    let (mut live, mut rate) = (0, None);
    for second in 0..12_u64 {
        // The gauge gains one 8192-token chunk every other second, like the server's.
        if second % 2 == 1 {
            live += 8192;
        }
        let text = mlx_serve_prefill(live, true, 100, 1.0);
        rate = history
            .observe("mlx-serve", &text, now + Duration::from_secs(second))
            .unwrap()
            .prefill_tps;
    }
    let rate = rate.unwrap();
    assert!(
        (3500.0..4700.0).contains(&rate),
        "{rate}: one second would read 0, 8192 or 16384"
    );
}

#[test]
fn a_finished_prefill_stays_as_the_last_sample_through_a_long_decode() {
    let now = Instant::now();
    let mut history = MetricsHistory::default();
    let mut poll = |second: u64, tokens: u64, seconds: f64| {
        let text = mlx_serve_prefill(0, false, tokens, seconds).replace("running 1", "running 0");
        let result = history
            .observe("mlx-serve", &text, now + Duration::from_secs(second))
            .unwrap();
        (result.prefill_tps, result.prefill_tps_live)
    };
    assert_eq!(poll(0, 0, 0.0), (None, false));
    assert_eq!(poll(1, 0, 0.0), (Some(0.0), true));
    let (rate, live) = poll(2, 500, 0.15);
    assert!(
        (rate.unwrap() - 3333.33).abs() < 1.0 && !live,
        "a finished request's speed is not live"
    );
    let (rate, live) = poll(4, 500, 0.15);
    assert!(rate.unwrap() > 3000.0 && !live, "still held two seconds on");
    let (rate, live) = poll(7, 500, 0.15);
    assert!(
        rate.unwrap() > 3000.0 && !live,
        "a decode running past the old hold keeps the last prefill"
    );
}

#[test]
fn an_idle_server_shows_its_cumulative_prefill_speed_after_a_long_gap() {
    let now = Instant::now();
    let mut history = MetricsHistory::default();
    for second in 0..3_u64 {
        let text = mlx_serve_prefill(0, false, 100, 1.0);
        let result = history
            .observe("mlx-serve", &text, now + Duration::from_secs(second))
            .unwrap();
        assert_eq!(
            (result.prefill_tps, result.prefill_tps_live),
            (Some(100.0), false),
            "second {second}: the cumulative speed is shown, not live"
        );
    }
    let text = mlx_serve_prefill(0, false, 100, 1.0);
    let later = history
        .observe("mlx-serve", &text, now + Duration::from_secs(60))
        .unwrap();
    assert_eq!(
        (later.prefill_tps, later.prefill_tps_live),
        (Some(100.0), false)
    );
}

#[test]
fn mlx_serve_sessions_name_the_model_and_list_only_running_requests() {
    let now = SystemTime::now();
    let mut result = LlmTelemetry::default();
    apply_mlx_serve_sessions(
        &mut result,
        &json!({"sessions":[
            {"model":"Qwen", "phase":"decode", "context_tokens":1500, "generated_tokens":300, "cached_tokens":1000, "request_id":7},
            {"model":"Qwen", "phase":"prefill", "context_tokens":900, "generated_tokens":0, "cached_tokens":0},
            {"model":"Qwen", "phase":"cached", "context_tokens":50000, "generated_tokens":0, "cached_tokens":50000}
        ]}),
        now,
        Instant::now(),
        &mut SessionSpeeds::default(),
    );
    assert_eq!(result.model.as_deref(), Some("Qwen"));
    let requests = &result.requests;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].id, "7");
    assert_eq!(
        (requests[0].prompt, requests[0].cached, requests[0].output),
        (1200, Some(1000), Some(300))
    );
    assert_eq!(requests[1].id, "slot-1");
    assert!(requests
        .iter()
        .all(|r| !r.completed && r.provider == "mlx-serve"));
    let cache = result.cache_interval_efficiency.unwrap();
    assert!(
        (cache - 1000.0 / 2100.0 * 100.0).abs() < 1e-9,
        "{cache}: live sessions carry the prefix reuse while no request completes"
    );
    assert_eq!(
        result.prompt_tokens, None,
        "two requests: no single-request totals"
    );
    let mut one = LlmTelemetry::default();
    apply_mlx_serve_sessions(
        &mut one,
        &json!({"sessions":[{"phase":"decode","context_tokens":50,"generated_tokens":20}]}),
        now,
        Instant::now(),
        &mut SessionSpeeds::default(),
    );
    assert_eq!((one.prompt_tokens, one.output_tokens), (Some(30), Some(20)));
    let mut none = LlmTelemetry::default();
    apply_mlx_serve_sessions(
        &mut none,
        &json!({}),
        now,
        Instant::now(),
        &mut SessionSpeeds::default(),
    );
    assert!(none.requests.is_empty() && none.model.is_none());
}

/// One poll of `/metrics.json` at `second`, returning each request's own speed.
fn session_speeds(
    speeds: &mut SessionSpeeds,
    start: Instant,
    second: u64,
    sessions: Value,
) -> Vec<Option<f64>> {
    let mut result = LlmTelemetry::default();
    apply_mlx_serve_sessions(
        &mut result,
        &json!({ "sessions": sessions }),
        SystemTime::now(),
        start + Duration::from_secs(second),
        speeds,
    );
    result
        .requests
        .iter()
        .map(|request| request.output_tps)
        .collect()
}

#[test]
fn mlx_serve_reports_each_requests_own_decode_speed() {
    let start = Instant::now();
    let mut speeds = SessionSpeeds::default();
    let decode = |id: u64, generated: u64| json!({"model":"Qwen","phase":"decode","context_tokens":1300 + generated,"generated_tokens":generated,"cached_tokens":0,"request_id":id});
    assert_eq!(
        session_speeds(&mut speeds, start, 0, json!([decode(7, 100)])),
        [None],
        "one sample measures nothing"
    );
    assert_eq!(
        session_speeds(&mut speeds, start, 2, json!([decode(7, 300)])),
        [Some(100.0)]
    );
    // Two requests each read their own count, not the server total.
    assert_eq!(
        session_speeds(&mut speeds, start, 3, json!([decode(7, 350), decode(8, 0)])),
        [Some(250.0 / 3.0), None]
    );
    assert_eq!(
        session_speeds(
            &mut speeds,
            start,
            4,
            json!([decode(7, 400), decode(8, 40)])
        ),
        [Some(75.0), Some(40.0)]
    );
    // The prompt is the request's identity: a reused id with a different prompt starts over.
    assert_eq!(
        session_speeds(
            &mut speeds,
            start,
            5,
            json!([{"phase":"decode","context_tokens":9000,"generated_tokens":10,"request_id":7}])
        ),
        [None]
    );
    // A synthetic slot id is positional, so it never carries one.
    let mut slots = SessionSpeeds::default();
    let slot = |generated: u64| json!([{"phase":"decode","context_tokens":50 + generated,"generated_tokens":generated}]);
    session_speeds(&mut slots, start, 0, slot(5));
    assert_eq!(session_speeds(&mut slots, start, 1, slot(105)), [None]);
    // A prefilling session has no decode speed, even once it has been seen before.
    let mut prefill = SessionSpeeds::default();
    let prefilling =
        json!([{"phase":"prefill","context_tokens":900,"generated_tokens":0,"request_id":9}]);
    session_speeds(&mut prefill, start, 0, prefilling.clone());
    assert_eq!(session_speeds(&mut prefill, start, 1, prefilling), [None]);
}

#[test]
fn a_session_count_that_moves_every_two_seconds_reads_steadily() {
    // The same refresh as the server gauge: 210 tokens at 2, 4 and 6 seconds, polled every second.
    let start = Instant::now();
    let mut speeds = SessionSpeeds::default();
    let rates: Vec<_> = [0_u64, 0, 210, 210, 420, 420, 630]
        .into_iter()
        .enumerate()
        .map(|(second, generated)| {
            session_speeds(
                &mut speeds,
                start,
                second as u64,
                json!([{"phase":"decode","context_tokens":1000 + generated,"generated_tokens":generated,"request_id":3}]),
            )[0]
        })
        .collect();
    assert_eq!(rates[..2], [None, None]);
    for rate in &rates[2..] {
        assert_eq!(*rate, Some(105.0), "{rates:?}");
    }
}

#[test]
fn mlx_serve_restart_drops_per_session_rate_history() {
    let start = Instant::now();
    let mut history = MetricsHistory::default();
    let metrics = |started: u64| {
        format!(
            "{}mlx_serve:process_start_time_seconds {started}\n",
            mlx_serve_decode(100, 1)
        )
    };
    let session = |generated: u64| {
        json!({"sessions":[{"phase":"decode","context_tokens":1000 + generated,
            "generated_tokens":generated,"request_id":7}]})
    };
    let mut poll = |second: u64, started: u64, generated: u64| {
        let at = start + Duration::from_secs(second);
        let mut result = history.observe("mlx-serve", &metrics(started), at).unwrap();
        history.apply_sessions(&mut result, &session(generated), SystemTime::now(), at);
        result.requests[0].output_tps
    };
    assert_eq!(poll(0, 1, 10), None);
    assert_eq!(poll(1, 1, 20), Some(10.0));
    // A new process may reuse the id and prompt length between two polls.
    assert_eq!(poll(2, 2, 30), None);
}

#[test]
fn prometheus_rates_need_two_fresh_matching_counter_sets() {
    let now = Instant::now();
    let mut history = MetricsHistory::default();
    let first = history.observe("vLLM", &vllm(100, 400), now).unwrap();
    assert_eq!(first.generation_tps, None);
    assert_eq!(first.active_requests, Some(1));
    assert_eq!(first.waiting_requests, Some(2));
    assert_eq!(first.prefix_hit_rate, Some(60.0));
    assert_eq!(first.model.as_deref(), Some("test model"));
    assert!(first
        .details
        .as_deref()
        .unwrap()
        .contains("2000 ms (cumulative)"));
    assert!(
        first.requests.is_empty(),
        "aggregate TTFT isn't per-request latency"
    );
    let second = history
        .observe("vLLM", &vllm(180, 600), now + Duration::from_secs(2))
        .unwrap();
    assert_eq!(second.generation_tps, Some(40.0));
    assert_eq!(second.prefill_tps, Some(100.0));
    assert!(second.generation_tps_live);
    assert!(
        !second.prefill_tps_live,
        "vLLM counts a prompt in one step: tokens per poll, not a live speed"
    );
    let reset = history
        .observe("vLLM", &vllm(1, 1), now + Duration::from_secs(3))
        .unwrap();
    assert_eq!(reset.generation_tps, None);
    let stale = history
        .observe("vLLM", &vllm(500, 500), now + Duration::from_secs(10))
        .unwrap();
    assert_eq!(stale.generation_tps, None);
    assert!(history.observe("vLLM", "<html>error</html>", now).is_none());
    assert!(history.observe("invalid", "", now).is_none());
    let changed = history
        .observe(
            "vLLM",
            &vllm(600, 600).replace("test model", "other"),
            now + Duration::from_secs(11),
        )
        .unwrap();
    assert_eq!(changed.generation_tps, None);
    let start = format!(
        "{}process_start_time_seconds 42\n",
        vllm(700, 700).replace("test model", "other")
    );
    assert_eq!(
        history
            .observe("vLLM", &start, now + Duration::from_secs(12))
            .unwrap()
            .generation_tps,
        None
    );
}

#[test]
fn prometheus_parser_handles_labels_escapes_resets_and_invalid_series() {
    assert_eq!(metric_sum("metric{label=\"with spaces and \\\"quotes\\\"\", n=\"1\"} 2 123\nmetric{n=\"2\"} 3\nmetric_other 99", "metric"), Some(5.0));
    for bad in ["NaN", "+Inf", "-1", "invalid"] {
        assert_eq!(
            metric_sum(
                &format!("metric{{id=\"a\"}} 2\nmetric{{id=\"b\"}} {bad}"),
                "metric"
            ),
            None
        );
    }
    assert_eq!(metric_sum("metric 2\nmetric 2", "metric"), None);
    for bad in [
        "metric{broken 2",
        "metric{id=bad} 2",
        "metric{id=\"unterminated} 2",
        "metric{id=\"a\" garbage} 2",
    ] {
        assert_eq!(metric_sum(bad, "metric"), None);
    }
    let old = Metrics::parse("tokens{id=\"a\"} 100\ntokens{id=\"b\"} 10");
    let new = Metrics::parse("tokens{id=\"a\"} 1\ntokens{id=\"b\"} 200");
    assert_eq!(
        new.delta(&old, "tokens"),
        None,
        "one reset cannot hide behind another series increasing"
    );
    assert_eq!(
        Metrics::parse("tokens{id=\"a\"} 150").delta(&old, "tokens"),
        None
    );
    assert_eq!(
        Metrics::parse("tokens{id=\"a\"} 150\ntokens{id=\"b\"} 30").delta(&old, "tokens"),
        Some(70.0)
    );
    assert_eq!(Metrics::parse("n 1.5").count("n"), None);
    assert_eq!(Metrics::parse("n 1.1").max_fraction("n"), None);
    assert_eq!(seconds_to_ms(0.125), Some(125));
    assert_eq!(seconds_to_ms(f64::INFINITY), None);
    assert_eq!(seconds_to_ms(-1.0), None);
}

#[test]
fn sglang_queues_and_cache_do_not_fabricate_request_usage() {
    let mut history = MetricsHistory::default();
    for (active, waiting, status) in [(0, 0, "idle"), (0, 1, "waiting"), (2, 0, "processing")] {
        let text = format!("sglang:num_running_reqs {active}\nsglang:num_queue_reqs {waiting}\nsglang:cache_hit_rate 0.7\nsglang:token_usage 0.4\n");
        let result = history.observe("SGLang", &text, Instant::now()).unwrap();
        assert_eq!(result.status.as_deref(), Some(status));
        assert_eq!(result.prefix_hit_rate, Some(70.0));
        assert_eq!(result.cache_efficiency, None);
        assert!(result.requests.is_empty());
        assert_eq!(result.prompt_tokens, None);
    }
    let result = history.observe("SGLang", "sglang:prompt_tokens_total 50\nsglang:cache_hit_rate{rank=\"0\"} 0.4\nsglang:cache_hit_rate{rank=\"1\"} 0.8", Instant::now()).unwrap();
    assert_eq!(result.prefix_hit_rate, None);
    assert_eq!(result.status.as_deref(), Some("running"));
}

#[test]
fn endpoint_supports_auth_prefix_chunked_body_and_refuses_bad_config() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut data = [0; 4096];
        let n = stream.read(&mut data).unwrap();
        let request = String::from_utf8_lossy(&data[..n]).to_ascii_lowercase();
        assert!(request.starts_with("get /proxy/v1/models http/1.1"));
        assert!(request.contains("authorization: bearer test-token"));
        stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n2\r\n{}\r\n0\r\n\r\n").unwrap();
    });
    let endpoint = Endpoint {
        url: Some(format!("http://127.0.0.1:{port}/proxy/")),
        api_key: Some("test-token".into()),
        allow_remote_auth: false,
    };
    assert_eq!(endpoint.get(1, "/v1/models").as_deref(), Ok("{}"));
    server.join().unwrap();
    for url in [
        "invalid",
        "ftp://localhost",
        "http://user:secret@localhost",
        "http://localhost?token=secret",
        "http://localhost/#bad",
    ] {
        assert!(Endpoint {
            url: Some(url.into()),
            api_key: None,
            allow_remote_auth: false,
        }
        .get(1, "/models")
        .is_err());
    }
    for key in ["", "abc\r\nHeader: injected"] {
        assert!(Endpoint {
            url: None,
            api_key: Some(key.into()),
            allow_remote_auth: false,
        }
        .get(1, "/models")
        .is_err());
    }
}

#[test]
fn remote_endpoints_are_distinguished_from_loopback() {
    for (url, remote) in [
        ("http://127.0.0.1:8000", false),
        ("http://localhost:8000", false),
        ("https://inference.example.net", true),
        ("http://192.0.2.10:8000", true),
    ] {
        assert_eq!(
            Endpoint {
                url: Some(url.into()),
                api_key: None,
                allow_remote_auth: false,
            }
            .is_remote(),
            remote
        );
    }
}

#[test]
fn cache_interval_uses_fresh_token_deltas_and_never_turns_capacity_into_reuse() {
    let now = Instant::now();
    let mut history = MetricsHistory::default();
    let first = history.observe("vLLM", &vllm(100, 400), now).unwrap();
    assert_eq!(first.cache_interval_efficiency, None);
    let next = vllm(150, 500)
        .replace("queries_total 100", "queries_total 200")
        .replace("hits_total 60", "hits_total 100");
    let result = history
        .observe("vLLM", &next, now + Duration::from_secs(1))
        .unwrap();
    assert_eq!(result.cache_interval_efficiency, Some(40.0));
    assert_eq!(result.cache_efficiency, Some(50.0));
    assert!(result.details.unwrap().contains("KV occupancy max 25.0%"));
    let reset = history
        .observe("vLLM", &vllm(200, 600), now + Duration::from_secs(2))
        .unwrap();
    assert_eq!(reset.cache_interval_efficiency, None);
    for malformed in [
        "metric{x=\"a\" 2",
        "metric{x=\"a\"}2",
        "metric{x=\"a\",x=\"b\"} 2",
    ] {
        assert_eq!(metric_sum(malformed, "metric"), None);
    }
}

#[test]
fn remote_credentials_require_the_existing_explicit_opt_in() {
    // The policy error itself: on macOS 127.0.0.2 reaches nothing, so any
    // error would pass while the guard was gone.
    let endpoint = Endpoint {
        url: Some("http://192.0.2.10:8080".into()),
        api_key: Some("test-token".into()),
        allow_remote_auth: false,
    };
    assert_eq!(
        endpoint.get(1, "/models"),
        Err(ProbeIssue::RemoteAuthBlocked)
    );
}

#[test]
fn http_errors_redirects_and_oversized_bodies_never_become_telemetry() {
    for response in [
        "HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\n\r\n{}".to_owned(),
        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/secret\r\nContent-Length: 0\r\n\r\n"
            .to_owned(),
        "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_owned(),
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            MAX_HTTP_RESPONSE_BYTES + 1,
            "a".repeat(MAX_HTTP_RESPONSE_BYTES + 1)
        ),
    ] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            let _ = stream.write_all(response.as_bytes());
        });
        assert!(Endpoint::default().get(port, "/models").is_err());
        server.join().unwrap();
    }
}

fn poll_at(
    history: &mut MetricsHistory,
    now: Instant,
    second: u64,
    text: &str,
) -> (Option<f64>, bool) {
    let r = history
        .observe("mlx-serve", text, now + Duration::from_secs(second))
        .unwrap();
    (r.prefill_tps, r.prefill_tps_live)
}

#[test]
fn mlx_serve_prefill_is_not_live_once_the_prefill_ends() {
    let (now, mut h) = (Instant::now(), MetricsHistory::default());
    for (t, live) in [(0, 0), (1, 4096), (2, 8192), (3, 12288), (4, 16384)] {
        poll_at(&mut h, now, t, &mlx_serve_prefill(live, true, 0, 0.0));
    }
    for t in 5..=15 {
        let (rate, live) = poll_at(&mut h, now, t, &mlx_serve_prefill(0, false, 0, 0.0));
        assert!(
            !(live && rate.unwrap_or(0.0) > 0.0),
            "t={t}: {rate:?} still live"
        );
    }
}

#[test]
fn mlx_serve_slow_chunked_prefill_reads_its_true_speed() {
    let (now, mut h) = (Instant::now(), MetricsHistory::default());
    // 512 tok/s: an 8192-token chunk lands every 16 s, polled every second.
    for t in 0..48_u64 {
        let live = (t / 16) * 8192;
        let (rate, is_live) = poll_at(&mut h, now, t, &mlx_serve_prefill(live, true, 0, 0.0));
        match rate {
            Some(rate) => assert!(is_live && (460.0..=565.0).contains(&rate), "t={t}: {rate}"),
            None => assert!(t < 16, "t={t}: no reading once a chunk has landed"),
        }
    }
}

#[test]
fn mlx_serve_prefill_reading_survives_a_long_decode_and_stays_at_its_end() {
    let (now, mut h) = (Instant::now(), MetricsHistory::default());
    for (t, live) in [(0, 0), (1, 8192), (2, 16384)] {
        poll_at(&mut h, now, t, &mlx_serve_prefill(live, true, 0, 0.0));
    }
    for t in 3..60 {
        let (rate, live) = poll_at(&mut h, now, t, &mlx_serve_prefill(0, false, 0, 0.0));
        assert!(
            !live && rate.is_some_and(|rate| (rate - 8192.0).abs() < 1.0),
            "t={t}: a decode running for a minute still shows the last prefill: {rate:?}"
        );
    }
    let (rate, live) = poll_at(&mut h, now, 60, &mlx_serve_prefill(0, false, 16384, 2.0));
    assert!(
        !live && rate.is_some_and(|rate| (rate - 8192.0).abs() < 1.0),
        "the finished request replaces it: {rate:?}"
    );
}

#[test]
fn mlx_serve_prefill_rate_ignores_the_raw_gauge_and_missing_series() {
    // A restart mid-prefill: the first poll has nothing to measure from.
    let (now, mut h) = (Instant::now(), MetricsHistory::default());
    poll_at(&mut h, now, 0, &mlx_serve_prefill(0, false, 50_000, 10.0));
    let (rate, live) = poll_at(&mut h, now, 1, &mlx_serve_prefill(4096, true, 0, 0.0));
    assert!(!(live && rate.is_some()), "restart reads {rate:?} live");
    // An older build without the finished-request counters reads like one with them.
    let full = |live: u64| mlx_serve_prefill(live, true, 0, 0.0);
    let bare = |live: u64| {
        format!("vllm:num_requests_running 1\nmlx_serve:generation_tokens_live 0\nmlx_serve:prefill_tokens_live {live}\nmlx_serve:requests_prefilling 1\n")
    };
    let (mut with, mut without) = (MetricsHistory::default(), MetricsHistory::default());
    for (t, live) in [(0, 0), (1, 8192), (2, 8192), (3, 16384)] {
        assert_eq!(
            poll_at(&mut with, now, t, &full(live)),
            poll_at(&mut without, now, t, &bare(live)),
            "t={t}"
        );
    }
    // No requests_prefilling gauge: a growing prefill is still live.
    let flagless = |live: u64| {
        format!("vllm:num_requests_running 1\nmlx_serve:generation_tokens_live 0\nmlx_serve:prefill_tokens_live {live}\nmlx_serve:prefill_tokens_total 0\nvllm:request_prefill_time_seconds_sum 0\n")
    };
    let mut h = MetricsHistory::default();
    let mut last = (None, false);
    for (t, live) in [(0, 0), (1, 4096), (2, 8192), (3, 12288)] {
        last = poll_at(&mut h, now, t, &flagless(live));
    }
    assert!(last.1 && last.0.is_some_and(|rate| rate > 0.0), "{last:?}");
}

#[test]
fn mlx_serve_fallback_ids_follow_the_running_request() {
    let now = SystemTime::now();
    let ids = |sessions: Value| {
        let mut r = LlmTelemetry::default();
        apply_mlx_serve_sessions(
            &mut r,
            &json!({ "sessions": sessions }),
            now,
            Instant::now(),
            &mut SessionSpeeds::default(),
        );
        r.requests.iter().map(|q| q.id.clone()).collect::<Vec<_>>()
    };
    let decode = |generated: u64| json!({"phase":"decode","context_tokens":700,"generated_tokens":generated});
    let cached = json!({"phase":"cached","context_tokens":50000});
    let with_cached = ids(json!([cached, decode(30), decode(40)]));
    let evicted = ids(json!([decode(31), decode(41)]));
    assert_eq!(with_cached.len(), 2);
    assert_ne!(with_cached[0], with_cached[1]);
    assert_eq!(
        with_cached, evicted,
        "a cached row ahead of a request must not rename it"
    );
    // The server's id 0 means "none", not a request named "0".
    let zero = ids(json!([
        {"phase":"decode","context_tokens":700,"generated_tokens":3,"request_id":0},
        {"phase":"decode","context_tokens":900,"generated_tokens":5,"request_id":0}
    ]));
    assert_ne!(zero[0], zero[1]);
}

/// Only the decode gauge and its running count: the server refreshes the gauge once every two seconds.
fn mlx_serve_decode(live: u64, running: u64) -> String {
    format!("vllm:num_requests_running {running}\nmlx_serve:generation_tokens_live {live}\nmlx_serve:prefill_tokens_live 0\nmlx_serve:requests_prefilling 0\n")
}

#[test]
fn decode_reads_the_gauge_step_not_the_poll_interval() {
    let now = Instant::now();
    let mut history = MetricsHistory::default();
    // 210 tokens per server refresh at 2, 4 and 6 seconds, polled every second.
    let mut rates = Vec::new();
    for (second, live) in [0_u64, 0, 210, 210, 420, 420, 630].into_iter().enumerate() {
        rates.push(
            history
                .observe(
                    "mlx-serve",
                    &mlx_serve_decode(live, 1),
                    now + Duration::from_secs(second as u64),
                )
                .unwrap()
                .generation_tps,
        );
    }
    assert_eq!(
        (rates[0], rates[1]),
        (None, None),
        "nothing before the first step"
    );
    // A delta over one poll read 0 or 210 depending on where it landed; the step reads 105 either way.
    assert!(
        rates[2..].iter().all(|rate| *rate == Some(105.0)),
        "{rates:?}"
    );
}

#[test]
fn decode_speed_holds_across_poll_gaps_and_reads_zero_when_the_gauge_stops() {
    let now = Instant::now();
    let mut history = MetricsHistory::default();
    let mut poll = |second: u64, live: u64, running: u64| {
        let result = history
            .observe(
                "mlx-serve",
                &mlx_serve_decode(live, running),
                now + Duration::from_secs(second),
            )
            .unwrap();
        (result.generation_tps, result.generation_tps_live)
    };
    assert_eq!(poll(0, 0, 1), (None, false));
    assert_eq!(poll(2, 210, 1), (Some(105.0), true));
    assert_eq!(
        poll(3, 210, 1),
        (Some(105.0), true),
        "a poll that catches no step keeps the speed"
    );
    // The request ends between refreshes: the gauge stops, and the reading follows it down.
    assert_eq!(poll(4, 210, 0), (Some(105.0), true));
    assert_eq!(poll(7, 210, 0), (Some(0.0), true));
    assert_eq!(
        poll(10, 210, 1),
        (None, false),
        "a new request has not been measured yet"
    );
    assert_eq!(poll(12, 420, 1), (Some(105.0), true));
}

#[test]
fn a_gauge_drop_restarts_the_decode_window() {
    let now = Instant::now();
    let mut history = MetricsHistory::default();
    let mut poll = |second: u64, live: u64| {
        history
            .observe(
                "mlx-serve",
                &mlx_serve_decode(live, 1),
                now + Duration::from_secs(second),
            )
            .unwrap()
            .generation_tps
    };
    assert_eq!(poll(0, 0), None);
    assert_eq!(poll(2, 210), Some(105.0));
    assert_eq!(
        poll(4, 5),
        None,
        "a restarted server's gauge carries no speed"
    );
    assert_eq!(poll(6, 215), Some(105.0));
}

#[test]
fn mlx_serve_session_cache_reuse_requires_complete_counts() {
    let known = json!({"phase":"decode", "context_tokens":120,
        "generated_tokens":20, "request_id":1, "cached_tokens":80});
    let unknown = json!({"phase":"decode", "context_tokens":120,
        "generated_tokens":20, "request_id":2});
    for (sessions, expected) in [
        (vec![unknown.clone()], None),
        (vec![known.clone(), unknown], None),
        (vec![known], Some(80.0)),
    ] {
        let mut result = LlmTelemetry::default();
        apply_mlx_serve_sessions(
            &mut result,
            &json!({"sessions": sessions}),
            SystemTime::now(),
            Instant::now(),
            &mut SessionSpeeds::default(),
        );
        assert_eq!(result.cache_interval_efficiency, expected);
    }
}

#[test]
fn tensorfold_metrics_report_queues_cache_memory_and_completion_throughput() {
    let body = |output, prompt| {
        format!(
            "tensorfold:requests_running 2\n\
         tensorfold:requests_waiting 1\n\
         tensorfold:generation_tokens_total {output}\n\
         tensorfold:prompt_tokens_total {prompt}\n\
         tensorfold:prompt_tokens_cached_total 40\n\
         tensorfold:device_memory_bytes 1024\n\
         tensorfold:kv_cache_usage_ratio{{pool=\"0\"}} 0.25\n"
        )
    };
    let now = Instant::now();
    let mut history = MetricsHistory::default();
    let first = history.observe("TensorFold", &body(10, 100), now).unwrap();
    assert_eq!(first.provider.as_deref(), Some("TensorFold"));
    assert_eq!(first.active_requests, Some(2));
    assert_eq!(first.waiting_requests, Some(1));
    assert_eq!(first.model_memory, Some(1024));
    assert_eq!(first.prefix_hit_rate, Some(40.0));
    assert_eq!(first.generation_tps, None);
    assert!(first.requests.is_empty());
    assert!(first.details.unwrap().contains("25.0%"));
    let next = history
        .observe("TensorFold", &body(50, 300), now + Duration::from_secs(2))
        .unwrap();
    assert_eq!(next.generation_tps, Some(20.0));
    assert_eq!(next.prefill_tps, Some(100.0));
    assert!(
        !next.generation_tps_live,
        "completion bursts are not live decode speed"
    );
    assert!(
        !next.prefill_tps_live,
        "completion bursts are not live prefill speed"
    );
    let reset = history
        .observe("TensorFold", &body(0, 0), now + Duration::from_secs(3))
        .unwrap();
    assert_eq!(reset.generation_tps, None);
    assert_eq!(reset.prefill_tps, None);
    assert_eq!(reset.prefix_hit_rate, None);
    assert!(history
        .observe("TensorFold", "unrelated 1\n", now)
        .is_none());
}

#[test]
fn strata_metrics_report_live_gauges_queues_cache_and_memory() {
    let body = |tok_s, prefill_s, output, prompt| {
        format!(
            "vllm:num_requests_running 1\n\
         vllm:num_requests_waiting 2\n\
         vllm:generation_tokens_total{{model_name=\"qwen3.8\"}} {output}\n\
         vllm:prompt_tokens_total{{model_name=\"qwen3.8\"}} {prompt}\n\
         vllm:prefix_cache_hits_total 40\n\
         vllm:prefix_cache_queries_total {prompt}\n\
         vllm:kv_cache_usage_perc{{model_name=\"qwen3.8\"}} 0.3\n\
         vllm:time_to_first_token_seconds_sum 1.5\n\
         vllm:time_to_first_token_seconds_count 3\n\
         strata:live_tok_s {tok_s}\n\
         strata:live_prefill_tok_s_mean {prefill_s}\n\
         strata:live_prompt_read 512\n\
         strata:gpu_mem_used_bytes{{gpu=\"0\"}} 4294967296\n\
         strata:gpu_mem_used_bytes{{gpu=\"1\"}} 4294967296\n"
        )
    };
    let now = Instant::now();
    let mut history = MetricsHistory::default();
    let first = history
        .observe("Strata", &body(25.5, 200.0, 10, 100), now)
        .unwrap();
    assert_eq!(first.provider.as_deref(), Some("Strata"));
    assert_eq!(first.active_requests, Some(1));
    assert_eq!(first.waiting_requests, Some(2));
    assert_eq!(first.model.as_deref(), Some("qwen3.8"));
    assert_eq!(first.model_memory, Some(2 * 4294967296));
    assert_eq!(first.prefix_hit_rate, Some(40.0));
    assert_eq!(first.generation_tps, Some(25.5));
    assert!(first.generation_tps_live, "the engine gauge is a live rate");
    assert_eq!(first.prefill_tps, Some(200.0));
    assert!(first.prefill_tps_live);
    let details = first.details.unwrap();
    assert!(details.contains("engine live gauges"));
    assert!(details.contains("30.0%"));
    assert!(details.contains("server mean TTFT 500 ms"));
    assert!(details.contains("prefill 512 tokens read"));
    let next = history
        .observe(
            "Strata",
            &body(0.0, 0.0, 50, 300),
            now + Duration::from_secs(2),
        )
        .unwrap();
    assert_eq!(next.generation_tps, Some(20.0));
    assert!(!next.generation_tps_live, "completion deltas are not live");
    assert_eq!(next.prefill_tps, Some(100.0));
    assert!(!next.prefill_tps_live);
    assert!(!next.details.unwrap().contains("engine live gauges"));
    let reset = history
        .observe(
            "Strata",
            &body(0.0, 0.0, 0, 0),
            now + Duration::from_secs(3),
        )
        .unwrap();
    assert_eq!(reset.generation_tps, None);
    assert_eq!(reset.prefill_tps, None);
    assert_eq!(reset.prefix_hit_rate, None);
    assert!(history.observe("Strata", "unrelated 1\n", now).is_none());
}

#[test]
fn tensorfold_on_a_mac_reports_model_memory_from_the_process_footprint() {
    let body = "tensorfold:requests_running 0\n\
                tensorfold:requests_waiting 0\n\
                tensorfold:process_footprint_bytes 2048\n";
    let result = MetricsHistory::default()
        .observe("TensorFold", body, Instant::now())
        .unwrap();
    assert_eq!(result.model_memory, Some(2048));
    let both = format!("{body}tensorfold:device_memory_bytes 1024\n");
    let result = MetricsHistory::default()
        .observe("TensorFold", &both, Instant::now())
        .unwrap();
    assert_eq!(result.model_memory, Some(1024), "CUDA device memory wins");
}

#[test]
fn sglang_hit_rate_is_cached_over_prompt_tokens_and_its_rates_are_not_live() {
    let body = |output, prompt, cached| {
        format!(
            "sglang:num_running_reqs{{dp=\"0\"}} 1\n\
             sglang:num_running_reqs{{dp=\"1\"}} 0\n\
             sglang:num_queue_reqs 0\n\
             sglang:generation_tokens_total {output}\n\
             sglang:prompt_tokens_total {prompt}\n\
             sglang:cached_tokens_total {cached}\n"
        )
    };
    let now = Instant::now();
    let mut history = MetricsHistory::default();
    let first = history.observe("SGLang", &body(10, 200, 50), now).unwrap();
    assert_eq!(first.prefix_hit_rate, Some(25.0));
    let next = history
        .observe("SGLang", &body(50, 400, 150), now + Duration::from_secs(2))
        .unwrap();
    assert_eq!(next.generation_tps, Some(20.0));
    assert_eq!(next.cache_interval_efficiency, Some(50.0));
    assert!(
        !next.generation_tps_live,
        "SGLang counts tokens at completion"
    );
    assert!(!next.prefill_tps_live);
    assert!(next
        .details
        .unwrap()
        .starts_with("throughput: completed-request"));
}

#[test]
fn vllm_0_9_and_earlier_report_a_hit_rate_from_the_gpu_prefix_cache_names() {
    let old = "vllm:num_requests_running 1\n\
               vllm:num_requests_waiting 0\n\
               vllm:gpu_prefix_cache_hits_total 30\n\
               vllm:gpu_prefix_cache_queries_total 120\n";
    let result = MetricsHistory::default()
        .observe("vLLM", old, Instant::now())
        .unwrap();
    assert_eq!(result.prefix_hit_rate, Some(25.0));
    let both =
        format!("{old}vllm:prefix_cache_hits_total 10\nvllm:prefix_cache_queries_total 100\n");
    let result = MetricsHistory::default()
        .observe("vLLM", &both, Instant::now())
        .unwrap();
    assert_eq!(result.prefix_hit_rate, Some(10.0), "current names win");
}

#[test]
fn vllm_prefill_rate_excludes_cached_tokens_and_is_not_live() {
    let start = Instant::now();
    let mut history = MetricsHistory::default();
    let scrape = |prompt: u64, cached: u64| {
        format!(
            "vllm:num_requests_running 1\nvllm:num_requests_waiting 0\n\
vllm:generation_tokens_total 10\nvllm:prompt_tokens_total {prompt}\n\
vllm:prompt_tokens_cached_total {cached}\n"
        )
    };
    history.observe("vLLM", &scrape(1000, 400), start);
    let result = history
        .observe("vLLM", &scrape(3000, 1400), start + Duration::from_secs(2))
        .unwrap();
    assert_eq!(result.prefill_tps, Some(500.0));
    assert!(!result.prefill_tps_live);
}

// Fixtures built from each runtime's tagged source or documentation; the
// files say where each came from.
const VLLM_0: &str = include_str!("fixtures/native_runtimes/vllm_v0.33.0rc3_0.prom");
const VLLM_1: &str = include_str!("fixtures/native_runtimes/vllm_v0.33.0rc3_1.prom");
const SGLANG_0: &str = include_str!("fixtures/native_runtimes/sglang_v0.5.23_0.prom");
const SGLANG_1: &str = include_str!("fixtures/native_runtimes/sglang_v0.5.23_1.prom");
const TENSORFOLD_0: &str = include_str!("fixtures/native_runtimes/tensorfold_v1.2.9_0.prom");
const TENSORFOLD_1: &str = include_str!("fixtures/native_runtimes/tensorfold_v1.2.9_1.prom");
/// Ollama v0.40.16: `/api/ps` in `api/types.go`'s `ProcessModelResponse`
/// shape (with `context_length` and `runner`, which `PsHandler` in
/// `server/routes.go` fills), describing the model of `docs/api.md`'s
/// `/api/show` example, which is the next fixture.
const OLLAMA_PS: &str = include_str!("fixtures/native_runtimes/ollama_v0.40.16_ps.json");
const OLLAMA_SHOW: &str = include_str!("fixtures/native_runtimes/ollama_v0.40.16_show.json");
/// A safetensors model on Ollama's MLX runner: the keys `buildModelInfo` in
/// `server/show_safetensors.go` at v0.40.16 writes from `config.json`.
const OLLAMA_SHOW_MLX: &str =
    include_str!("fixtures/native_runtimes/ollama_v0.40.16_show_safetensors.json");
/// LM Studio's documented `GET /api/v1/models` example
/// (lmstudio.ai/docs/developer/rest/list, read 2026-11-15).
const LM_STUDIO_MODELS: &str =
    include_str!("fixtures/native_runtimes/lmstudio_rest_v1_models.json");

fn close(value: Option<f64>, expected: f64) -> bool {
    value.is_some_and(|value| (value - expected).abs() < 1e-6)
}

/// Two scrapes of `provider`, `seconds` apart.
fn scrapes(
    provider: &str,
    first: &str,
    second: &str,
    seconds: u64,
) -> (LlmTelemetry, LlmTelemetry) {
    let start = Instant::now();
    let mut history = MetricsHistory::default();
    let first = history.observe(provider, first, start).unwrap();
    let second = history
        .observe(provider, second, start + Duration::from_secs(seconds))
        .unwrap();
    (first, second)
}

#[test]
fn vllm_reads_preemptions_draft_kv_room_and_the_waits_of_each_interval() {
    let (first, second) = scrapes("vLLM", VLLM_0, VLLM_1, 2);
    assert_eq!(first.preemptions_total, Some(3));
    // 108,544 tokens of capacity, 62% held.
    assert_eq!(first.kv_free_tokens, Some(41_247));
    assert_eq!(first.kv_free_bytes, None, "vLLM gives no bytes per token");
    assert_eq!(first.model_facts.kv_dtype.as_deref(), Some("fp8_e4m3"));
    assert_eq!(first.model_facts.draft, Some(true));
    assert_eq!(
        first.tokens_per_pass, None,
        "one scrape measures no interval"
    );
    assert!(!first
        .details
        .as_deref()
        .unwrap()
        .contains("since the last poll"));
    assert_eq!(second.preemptions_total, Some(4));
    assert_eq!(second.generation_tps, Some(40.0));
    // 57 tokens accepted over 30 drafts, plus the target's own.
    assert!(
        close(second.tokens_per_pass, 2.9),
        "{:?}",
        second.tokens_per_pass
    );
    assert_eq!(
        second.details.as_deref(),
        Some(
            "throughput: server token deltas · KV occupancy max 62.0% · server mean TTFT \
             819 ms (cumulative) · since the last poll: queue wait 1.2 s, 24 ms between \
             tokens, 63% of draft tokens accepted"
        )
    );
}

#[test]
fn vllm_without_cache_config_or_a_draft_keeps_unknown_unknown() {
    let bare = |text: &str| {
        text.lines()
            .filter(|line| !line.contains("cache_config_info") && !line.contains("spec_decode"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let (first, second) = scrapes("vLLM", &bare(VLLM_0), &bare(VLLM_1), 2);
    assert_eq!(first.kv_free_tokens, None, "no capacity, no room");
    assert_eq!(first.model_facts.kv_dtype, None);
    assert_eq!(
        first.model_facts.draft,
        Some(false),
        "a V1 engine without a draft"
    );
    assert_eq!(second.tokens_per_pass, None);
    assert!(!second.details.unwrap().contains("draft tokens"));
    // An engine that names its usage gauge the old way says nothing of a draft.
    let old = bare(VLLM_0).replace("vllm:kv_cache_usage_perc", "vllm:gpu_cache_usage_perc");
    let old = MetricsHistory::default()
        .observe("vLLM", &old, Instant::now())
        .unwrap();
    assert_eq!(old.model_facts.draft, None);
    // An `auto` cache keeps the model's dtype, which vLLM does not name here.
    let auto = VLLM_0.replace("cache_dtype=\"fp8_e4m3\"", "cache_dtype=\"auto\"");
    let auto = MetricsHistory::default()
        .observe("vLLM", &auto, Instant::now())
        .unwrap();
    assert_eq!(auto.model_facts.kv_dtype, None);
}

#[test]
fn vllm_kv_room_is_counted_in_blocks_before_tokens_and_on_the_fullest_engine() {
    let engine = |engine: u8, used: f64, config: &str| {
        format!(
            "vllm:num_requests_running{{model_name=\"m\",engine=\"{engine}\"}} 1\n\
             vllm:kv_cache_usage_perc{{model_name=\"m\",engine=\"{engine}\"}} {used}\n\
             vllm:cache_config_info{{block_size=\"16\",{config}engine=\"{engine}\"}} 1.0\n"
        )
    };
    let room = |text: &str| {
        MetricsHistory::default()
            .observe("vLLM", text, Instant::now())
            .unwrap()
            .kv_free_tokens
    };
    // An older vLLM names blocks, not tokens: 1,000 blocks of 16, a quarter held.
    assert_eq!(
        room(&engine(0, 0.25, "num_gpu_blocks=\"1000\",")),
        Some(12_000)
    );
    // `None` is how Python writes a count not yet known.
    assert_eq!(
        room(&engine(
            0,
            0.25,
            "kv_cache_size_tokens=\"None\",num_gpu_blocks=\"None\","
        )),
        None
    );
    // Two engines: the one with the least room decides.
    let two = format!(
        "{}{}",
        engine(0, 0.5, "kv_cache_size_tokens=\"10000\","),
        engine(1, 0.9, "kv_cache_size_tokens=\"10000\",")
    );
    assert_eq!(room(&two), Some(1_000));
    // An engine without its config is unknown, not free.
    let orphan = format!(
        "{}vllm:kv_cache_usage_perc{{model_name=\"m\",engine=\"1\"}} 0.1\n",
        engine(0, 0.5, "kv_cache_size_tokens=\"10000\",")
    );
    assert_eq!(room(&orphan), None);
    let full = engine(0, 1.0, "kv_cache_size_tokens=\"10000\",");
    assert_eq!(room(&full), Some(0), "a full cache is a real zero");
    let broken = engine(0, 1.5, "kv_cache_size_tokens=\"10000\",");
    assert_eq!(room(&broken), None, "a share past 100% is no reading");
}

#[test]
fn interval_figures_survive_a_counter_reset_without_negative_or_huge_values() {
    // The runtime restarts between two polls: every counter falls.
    for (provider, before, after) in [
        ("vLLM", VLLM_1, VLLM_0),
        ("SGLang", SGLANG_1, SGLANG_0),
        ("TensorFold", TENSORFOLD_1, TENSORFOLD_0),
    ] {
        let start = Instant::now();
        let mut history = MetricsHistory::default();
        history.observe(provider, before, start).unwrap();
        let reset = history
            .observe(provider, after, start + Duration::from_secs(2))
            .unwrap();
        assert_eq!(reset.generation_tps, None, "{provider}");
        assert_eq!(reset.tokens_per_pass, None, "{provider}");
        let details = reset.details.unwrap();
        assert!(
            !details.contains("since the last poll"),
            "{provider}: {details}"
        );
        assert!(!details.contains('-'), "{provider}: {details}");
        // The next interval measures again, from the restarted counters.
        let next = history
            .observe(provider, before, start + Duration::from_secs(4))
            .unwrap();
        let rate = next.generation_tps.unwrap();
        assert!((0.0..1000.0).contains(&rate), "{provider}: {rate}");
    }
}

#[test]
fn sglang_reads_a_live_decode_speed_kv_room_retractions_and_scheduler_time() {
    let (first, second) = scrapes("SGLang", SGLANG_0, SGLANG_1, 2);
    assert_eq!(
        first.preemptions_total,
        Some(2),
        "retractions are its preemptions"
    );
    // Free slots plus cached prefixes it would evict.
    assert_eq!(first.kv_free_tokens, Some(240_000));
    let facts = &first.model_facts;
    assert_eq!(facts.kv_bytes_per_token_reported, Some(147_456));
    assert_eq!(first.kv_free_bytes, Some(240_000 * 147_456));
    assert_eq!(facts.context_configured, Some(40_960));
    assert_eq!(facts.loaded_bytes, Some(16_384_000_000));
    assert_eq!(facts.draft, Some(true));
    assert_eq!(
        first.generation_tps, None,
        "one scrape measures no interval"
    );
    assert!(!first.generation_tps_live);
    assert!(first
        .details
        .as_deref()
        .unwrap()
        .starts_with("throughput: live decode tokens, prefill at completion"));
    assert_eq!(second.generation_tps, Some(85.0));
    assert!(second.generation_tps_live, "decode is counted every step");
    assert!(
        !second.prefill_tps_live,
        "prompts still count at completion"
    );
    assert!(close(second.tokens_per_pass, 3.1));
    assert_eq!(second.kv_free_tokens, Some(239_830));
    assert_eq!(
        second.details.as_deref(),
        Some(
            "throughput: live decode tokens, prefill at completion · KV occupancy max 40.0% \
             · server mean TTFT 300 ms (cumulative) · since the last poll: queue wait 800 ms, \
             31 ms between tokens, 4.1k cached tokens evicted · last 2 s: GPU busy 82%, \
             scheduler idle 3%, scheduler CPU 41%"
        )
    );
}

#[test]
fn sglang_without_the_decode_counter_uses_its_decode_step_gauge() {
    let older = |text: &str, running: u8| {
        text.lines()
            .filter(|line| !line.contains("realtime_tokens_total"))
            .map(|line| {
                if line.starts_with("sglang:num_running_reqs{") {
                    line.replace("} 1.0", &format!("}} {running}.0"))
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let busy = MetricsHistory::default()
        .observe("SGLang", &older(SGLANG_0, 1), Instant::now())
        .unwrap();
    assert_eq!(busy.generation_tps, Some(84.6));
    assert!(busy.generation_tps_live);
    assert!(close(busy.tokens_per_pass, 3.1));
    assert!(busy
        .details
        .unwrap()
        .starts_with("throughput: engine decode gauge, prefill at completion"));
    // The gauge keeps its last speed for half a minute after decoding stops.
    let idle = MetricsHistory::default()
        .observe("SGLang", &older(SGLANG_0, 0), Instant::now())
        .unwrap();
    assert_eq!(idle.generation_tps, Some(0.0));
    assert_eq!(idle.tokens_per_pass, None, "no decoding, no pass");
    // Neither series: the completion counters' averages, not live.
    let oldest: String = older(SGLANG_0, 1)
        .lines()
        .filter(|line| !line.contains("gen_throughput"))
        .collect::<Vec<_>>()
        .join("\n");
    let oldest = MetricsHistory::default()
        .observe("SGLang", &oldest, Instant::now())
        .unwrap();
    assert!(!oldest.generation_tps_live);
    assert!(oldest
        .details
        .unwrap()
        .starts_with("throughput: completed-request token deltas"));
}

#[test]
fn sglang_hybrid_pools_and_missing_gauges_leave_kv_figures_unknown() {
    let swa = format!("{SGLANG_0}sglang:max_total_num_tokens_swa{{model_name=\"m\"}} 100000.0\n");
    let hybrid = MetricsHistory::default()
        .observe("SGLang", &swa, Instant::now())
        .unwrap();
    assert_eq!(hybrid.kv_free_tokens, Some(240_000));
    assert_eq!(hybrid.model_facts.kv_bytes_per_token_reported, None);
    assert_eq!(hybrid.kv_free_bytes, None);
    let mamba = SGLANG_0.replace(
        "sglang:mamba_available_tokens{model_name=\"Qwen/Qwen3-8B\",engine_type=\"unified\",tp_rank=\"0\",pp_rank=\"0\",moe_ep_rank=\"0\"} 0.0",
        "sglang:mamba_available_tokens{model_name=\"Qwen/Qwen3-8B\",engine_type=\"unified\",tp_rank=\"0\",pp_rank=\"0\",moe_ep_rank=\"0\"} 64.0",
    );
    let mamba = MetricsHistory::default()
        .observe("SGLang", &mamba, Instant::now())
        .unwrap();
    assert_eq!(mamba.model_facts.kv_bytes_per_token_reported, None);
    let bare: String = SGLANG_0
        .lines()
        .filter(|line| !line.contains("evictable") && !line.contains("retracted"))
        .collect::<Vec<_>>()
        .join("\n");
    let bare = MetricsHistory::default()
        .observe("SGLang", &bare, Instant::now())
        .unwrap();
    assert_eq!(
        bare.kv_free_tokens, None,
        "free slots alone understate the room"
    );
    assert_eq!(bare.preemptions_total, None);
    let unreadable = SGLANG_0.replace("} 150000.0", "} NaN");
    let unreadable = MetricsHistory::default()
        .observe("SGLang", &unreadable, Instant::now())
        .unwrap();
    assert_eq!(unreadable.kv_free_tokens, None);
    // A server without a draft reads 0 until it decodes: no draft is not known.
    let plain = SGLANG_0.replace("} 3.1", "} 0.0");
    let plain = MetricsHistory::default()
        .observe("SGLang", &plain, Instant::now())
        .unwrap();
    assert_eq!(plain.model_facts.draft, None);
}

#[test]
fn sglang_scheduler_time_leaves_out_late_lumps_and_keeps_half_a_minute() {
    let start = Instant::now();
    let mut history = MetricsHistory::default();
    let at = |seconds: u64| start + Duration::from_secs(seconds);
    history.observe("SGLang", SGLANG_0, at(0)).unwrap();
    // A minute of idle credited at once when work resumes: not this poll's.
    let lump = SGLANG_1.replace("} 5000.06", "} 5060.0");
    let lumped = history.observe("SGLang", &lump, at(2)).unwrap();
    let details = lumped.details.unwrap();
    assert!(
        details.contains("last 2 s: GPU busy 82%, scheduler CPU 41%"),
        "{details}"
    );
    assert!(!details.contains("idle"), "{details}");
    // Thirty seconds on, with no interval to measure, the window has emptied.
    let later = history.observe("SGLang", SGLANG_1, at(40)).unwrap();
    assert!(!later.details.unwrap().contains("last "));
    // Without the device timer SGLang does not time the GPU.
    let untimed = |text: &str| {
        text.lines()
            .filter(|line| !line.contains("forward_execution"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let (_, second) = scrapes("SGLang", &untimed(SGLANG_0), &untimed(SGLANG_1), 2);
    let details = second.details.unwrap();
    assert!(
        details.ends_with("last 2 s: scheduler idle 3%, scheduler CPU 41%"),
        "{details}"
    );
}

#[test]
fn tensorfold_reads_a_live_decode_speed_its_draft_preemptions_and_refusals() {
    let (first, second) = scrapes("TensorFold", TENSORFOLD_0, TENSORFOLD_1, 2);
    assert_eq!(first.preemptions_total, Some(1));
    assert_eq!(first.model_facts.draft, Some(true));
    assert_eq!(
        first.generation_tps, None,
        "one scrape measures no interval"
    );
    let details = first.details.unwrap();
    assert!(
        details.starts_with("throughput: live decode tokens, prefill at completion"),
        "{details}"
    );
    assert!(
        details.ends_with("15 requests refused admission since the server started"),
        "{details}"
    );
    // A request finished between the polls: its 380 tokens moved from the
    // running streams to the finished total, and a new stream holds 40.
    assert_eq!(second.generation_tps, Some(60.0));
    assert!(second.generation_tps_live);
    assert!(!second.prefill_tps_live);
    // It kept 234 drafted tokens over 145 rounds.
    assert!(close(second.tokens_per_pass, 1.0 + 234.0 / 145.0));
    assert_eq!(second.model_memory, Some(21_474_836_480));
    // Without the live gauge, completion bursts stay averages.
    let older = |text: &str| {
        text.lines()
            .filter(|line| !line.contains("generation_tokens_running"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let (_, old) = scrapes("TensorFold", &older(TENSORFOLD_0), &older(TENSORFOLD_1), 2);
    assert_eq!(old.generation_tps, Some(190.0));
    assert!(!old.generation_tps_live);
}

#[test]
fn tensorfold_says_it_has_no_draft_only_after_requests_finish() {
    let undrafted = |text: &str| {
        text.replace(
            "tensorfold:mtp_drafted_total 6000",
            "tensorfold:mtp_drafted_total 0",
        )
        .replace(
            "tensorfold:mtp_accepted_total 3100",
            "tensorfold:mtp_accepted_total 0",
        )
    };
    let none = MetricsHistory::default()
        .observe("TensorFold", &undrafted(TENSORFOLD_0), Instant::now())
        .unwrap();
    assert_eq!(none.model_facts.draft, Some(false));
    let fresh = undrafted(TENSORFOLD_0).replace(
        "tensorfold:generation_tokens_total 5020",
        "tensorfold:generation_tokens_total 10",
    );
    let fresh = MetricsHistory::default()
        .observe("TensorFold", &fresh, Instant::now())
        .unwrap();
    assert_eq!(fresh.model_facts.draft, None, "too little finished to tell");
    // Nothing refused, nothing said.
    let open = TENSORFOLD_0
        .replace("} 3\n", "} 0\n")
        .replace("} 5\n", "} 0\n")
        .replace("} 7\n", "} 0\n");
    let open = MetricsHistory::default()
        .observe("TensorFold", &open, Instant::now())
        .unwrap();
    assert!(!open.details.unwrap().contains("refused"));
}

#[test]
fn ollama_show_facts_follow_the_model_file_and_stay_unknown_when_it_cannot_say() {
    let show: Value = serde_json::from_str(OLLAMA_SHOW).unwrap();
    let facts = show_facts(&show).unwrap();
    assert_eq!(facts.parameters, Some(8_030_261_248));
    assert_eq!(facts.context_trained, Some(8192));
    // A GGUF file of standard attention: every layer keeps keys and values,
    // and llama.cpp's own head dimension is the width over the heads.
    assert_eq!(
        (facts.kv_layers, facts.kv_heads, facts.head_dim),
        (Some(32), Some(8), Some(128))
    );
    assert_eq!((facts.experts, facts.experts_per_token), (None, None));
    assert_eq!(
        facts.kv_bytes_per_token(),
        None,
        "Ollama names no KV format"
    );
    let with = |key: &str, value: Value| {
        let mut show = show.clone();
        show["model_info"][key] = value;
        show_facts(&show).unwrap()
    };
    // Sliding windows: the layers that keep the whole context are unknown.
    let windowed = with("llama.attention.sliding_window", json!(4096));
    assert_eq!(
        (windowed.kv_layers, windowed.kv_heads, windowed.head_dim),
        (None, Some(8), Some(128))
    );
    // Latent attention caches what the runtime chooses.
    let latent = with("llama.attention.kv_lora_rank", json!(512));
    assert_eq!(
        (latent.kv_layers, latent.kv_heads, latent.head_dim),
        (None, None, None)
    );
    // Keys and values of different widths are not one head dimension.
    let mut uneven = show.clone();
    uneven["model_info"]["llama.attention.key_length"] = json!(192);
    uneven["model_info"]["llama.attention.value_length"] = json!(128);
    assert_eq!(show_facts(&uneven).unwrap().head_dim, None);
    let mut even = show.clone();
    even["model_info"]["llama.attention.key_length"] = json!(256);
    assert_eq!(show_facts(&even).unwrap().head_dim, Some(256));
    // A per-layer list of KV heads, omitted without `verbose`, is unknown.
    assert_eq!(
        with("llama.attention.head_count_kv", json!([])).kv_heads,
        None
    );
    // A mixture of experts: how many, and how many a token uses.
    let mut moe = show.clone();
    moe["model_info"]["llama.expert_count"] = json!(64);
    moe["model_info"]["llama.expert_used_count"] = json!(8);
    let moe = show_facts(&moe).unwrap();
    assert_eq!((moe.experts, moe.experts_per_token), (Some(64), Some(8)));
    assert_eq!(moe.kv_layers, Some(32), "experts keep standard attention");
    // A safetensors model: Ollama reads config.json, which names neither a
    // head dimension nor which layers are linear.
    let mlx = show_facts(&serde_json::from_str(OLLAMA_SHOW_MLX).unwrap()).unwrap();
    assert_eq!(mlx.parameters, Some(8_190_735_360));
    assert_eq!(mlx.context_trained, Some(40_960));
    assert_eq!(
        (mlx.kv_layers, mlx.kv_heads, mlx.head_dim),
        (None, Some(8), None)
    );
    assert_eq!(show_facts(&json!({"details":{}})), None);
    assert_eq!(
        show_facts(&json!({"model_info":{}})),
        Some(ModelFacts::default())
    );
}

#[test]
fn ollama_running_model_gets_its_runner_quantization_and_shown_facts() {
    let ps: Value = serde_json::from_str(&OLLAMA_PS.replace("2ae6f6dd", "5eed5eed")).unwrap();
    let digest = ps["models"][0]["digest"].as_str().unwrap().to_owned();
    let show: Value = serde_json::from_str(OLLAMA_SHOW).unwrap();
    OLLAMA_SHOWN
        .with_borrow_mut(|shown| shown.record(digest.clone(), show_facts(&show), Instant::now()));
    let result = ollama(&ps).unwrap();
    assert_eq!(
        result.details.as_deref(),
        Some("1 loaded models · resident VRAM 6.2 GiB · llama on llama.cpp")
    );
    let facts = &result.model_facts;
    assert_eq!(facts.quantization.as_deref(), Some("Q4_0"));
    assert_eq!(facts.context_configured, Some(8192));
    assert_eq!(facts.parameters, Some(8_030_261_248));
    assert_eq!(facts.kv_layers, Some(32));
    // Each part is said only when Ollama names it.
    let mut bare = ps.clone();
    bare["models"][0]["details"] = json!({});
    bare["models"][0]["runner"] = json!("mlx");
    let bare = ollama(&bare).unwrap();
    assert!(bare.details.unwrap().ends_with(" · on MLX"));
    assert_eq!(bare.model_facts.quantization, None);
    let mut family = ps.clone();
    family["models"][0]["runner"] = json!(null);
    assert!(ollama(&family)
        .unwrap()
        .details
        .unwrap()
        .ends_with(" · llama"));
    // A runner this does not know is named as Ollama names it.
    let mut unknown = ps.clone();
    unknown["models"][0]["runner"] = json!("vulkan");
    assert!(ollama(&unknown)
        .unwrap()
        .details
        .unwrap()
        .ends_with(" · llama on vulkan"));
    let mut other = ps.clone();
    other["models"][0]["runner"] = json!("ggml");
    other["models"][0]["digest"] = json!("not shown yet");
    let other = ollama(&other).unwrap();
    assert!(other.details.unwrap().ends_with(" · llama on GGML"));
    assert_eq!(other.model_facts.parameters, None, "not asked yet: unknown");
    // Two models: whose facts these would be is unknown.
    let two = json!({"models":[ps["models"][0].clone(), ps["models"][0].clone()]});
    assert_eq!(ollama(&two).unwrap().model_facts, ModelFacts::default());
}

#[test]
fn ollama_show_is_remembered_by_digest_and_retried_a_minute_after_a_failure() {
    let start = Instant::now();
    let mut shown = ShownModels::default();
    assert!(shown.wants("a", start));
    shown.record("a".into(), Some(ModelFacts::default()), start);
    assert!(!shown.wants("a", start + Duration::from_secs(3600)));
    shown.record("b".into(), None, start);
    assert!(!shown.wants("b", start + Duration::from_secs(59)));
    assert!(shown.wants("b", start + SHOW_RETRY));
    assert_eq!(shown.facts("b"), None);
    for digest in 0..SHOWN_MODELS {
        shown.record(digest.to_string(), Some(ModelFacts::default()), start);
    }
    assert!(shown.wants("a", start), "the oldest digest is forgotten");
    assert!(cloud_model("gpt-oss:120b-cloud"));
    assert!(cloud_model("gpt-oss:20b:cloud"));
    assert!(!cloud_model("llama3:8b"));
    assert!(!cloud_model("cloudy"));
}

#[test]
fn lm_studio_fills_quantization_experts_contexts_and_parallel_from_its_list() {
    let result = lm_studio(&serde_json::from_str(LM_STUDIO_MODELS).unwrap()).unwrap();
    assert_eq!(result.model.as_deref(), Some("google/gemma-4-26b-a4b"));
    let facts = &result.model_facts;
    assert_eq!(facts.quantization.as_deref(), Some("Q4_K_M"));
    assert_eq!(facts.experts_per_token, Some(8));
    assert_eq!(
        facts.experts, None,
        "LM Studio names experts used, not the total"
    );
    assert_eq!(facts.context_configured, Some(4096));
    assert_eq!(facts.context_trained, Some(262_144));
    assert_eq!(
        facts.bits_per_weight, None,
        "4 is the format's width, not what a pass reads"
    );
    assert_eq!(
        result.details.as_deref(),
        Some("1 loaded models · GGUF · weights 16.8 GiB · up to 4 requests at once")
    );
    // A quantization without a name reads as its bits.
    let unnamed = json!({"models":[{"key":"m","quantization":{"name":null,"bits_per_weight":4},
        "loaded_instances":[{"id":"m","config":{"num_experts":0}}]}]});
    let unnamed = lm_studio(&unnamed).unwrap();
    assert_eq!(unnamed.model_facts.quantization.as_deref(), Some("4-bit"));
    assert_eq!(
        unnamed.model_facts.experts_per_token, None,
        "0 keeps the model's own"
    );
    assert_eq!(unnamed.details.as_deref(), Some("1 loaded models"));
}

#[test]
fn native_runtime_lines_say_each_reading_once_and_only_when_known() {
    let sample = |provider: &str| Sample {
        llm_provider: provider.into(),
        llm_tokens_per_pass: Some(2.9),
        llm_kv_free_tokens: Some(41_247),
        llm_preemptions_total: Some(4),
        ..Sample::default()
    };
    assert_eq!(
        sample("vLLM").native_runtime_lines(),
        [
            "Speculative decoding: 2.9 tokens per verify pass",
            "KV cache: 41.2k tokens free · 4 requests preempted since the server started",
        ]
    );
    let sglang = Sample {
        llm_kv_free_tokens: Some(239_830),
        llm_kv_free_bytes: Some(239_830 * 147_456),
        llm_preemptions_total: Some(1),
        llm_tokens_per_pass: None,
        ..sample("SGLang")
    };
    assert_eq!(
        sglang.native_runtime_lines(),
        ["KV cache: 239.8k tokens free (32.9 GiB) · 1 request retracted since the server started"]
    );
    let quiet = Sample {
        llm_kv_free_tokens: None,
        llm_preemptions_total: Some(0),
        llm_tokens_per_pass: None,
        ..sample("TensorFold")
    };
    assert!(
        quiet.native_runtime_lines().is_empty(),
        "no preemption is no news"
    );
    for other in ["Ollama", "oMLX", "llama.cpp", "Splash"] {
        assert!(sample(other).native_runtime_lines().is_empty(), "{other}");
    }
}

/// A loopback server that answers every request with `answer(method, path)`
/// until it is told to stop, and returns each request it read as `METHOD
/// path body`, the body read in full.
fn recording(
    answer: impl Fn(&str, &str) -> (u16, String) + Send + 'static,
) -> (u16, thread::JoinHandle<Vec<String>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let mut seen = Vec::new();
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let (method, path, body) = read_request(&mut stream);
            if method == "STOP" {
                break;
            }
            seen.push(format!("{method} {path} {body}").trim_end().to_owned());
            let (status, reply) = answer(&method, &path);
            let _ = write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
        seen
    });
    (port, server)
}

fn read_request(stream: &mut TcpStream) -> (String, String, String) {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    let head = loop {
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break bytes.len(),
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
        }
    };
    let text = String::from_utf8_lossy(&bytes[..head]).into_owned();
    let length = text
        .lines()
        .find_map(|line| {
            let line = line.to_ascii_lowercase();
            line.strip_prefix("content-length:")?
                .trim()
                .parse::<usize>()
                .ok()
        })
        .unwrap_or(0);
    while bytes.len() < head + length {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
        }
    }
    let mut words = text.split_whitespace();
    let method = words.next().unwrap_or_default().to_owned();
    let path = words.next().unwrap_or_default().to_owned();
    let body = String::from_utf8_lossy(&bytes[head..]).into_owned();
    (method, path, body)
}

fn stop(port: u16, server: thread::JoinHandle<Vec<String>>) -> Vec<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(b"STOP / HTTP/1.1\r\n\r\n").unwrap();
    server.join().unwrap()
}

/// An adapter for `provider` polling `port`, as the sampler builds it.
fn adapter(provider: &str, port: u16) -> super::super::Adapter {
    let mut adapter = super::super::Adapter::new(None);
    adapter.configured = Some(provider.into());
    adapter.usage_file = None;
    adapter.port = Some(port);
    adapter.endpoint = Endpoint::default();
    adapter.config_error = None;
    assert!(adapter.selected(None), "{provider}");
    adapter
}

#[test]
fn magnitude_reads_the_catalogue_under_its_gateway_prefix() {
    let models =
        r#"{"object":"list","data":[{"id":"qwen3.6-35b-a3b:gguf:q4","name":"Qwen3.6 35B"}]}"#;
    let (port, server) = recording(move |_, path| match path {
        "/inference/v1/models" | "/v1/models" => (200, models.into()),
        _ => (404, "{}".into()),
    });
    let mut root = adapter("magnitude", port);
    let result = root.poll().unwrap();
    assert_eq!(result.provider.as_deref(), Some("Magnitude"));
    assert_eq!(result.model.as_deref(), Some("qwen3.6-35b-a3b:gguf:q4"));
    assert_eq!(result.status.as_deref(), Some("available"));
    // A URL copied from a client's base URL already ends in /inference.
    let mut prefixed = adapter("magnitude", port);
    prefixed.endpoint.url = Some(format!("http://127.0.0.1:{port}/inference/"));
    assert!(prefixed.poll().is_some());
    let requests = stop(port, server);
    assert_eq!(
        requests,
        ["GET /inference/v1/models", "GET /inference/v1/models"]
    );
}

#[test]
fn ollama_show_is_sent_once_per_digest_on_the_polled_endpoint() {
    let polled = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (port, server) = recording({
        let polled = polled.clone();
        move |method, path| match (method, path) {
            ("GET", "/api/ps") => {
                let poll = polled.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                // From the fourth poll a newly pulled model runs instead.
                let ps = OLLAMA_PS.to_owned();
                (
                    200,
                    if poll < 3 {
                        ps
                    } else {
                        ps.replace("2ae6f6dd", "0b1d2c3e")
                    },
                )
            }
            ("POST", "/api/show") => (200, OLLAMA_SHOW.to_owned()),
            _ => (404, "{}".into()),
        }
    });
    let mut adapter = adapter("ollama", port);
    let mut results = Vec::new();
    for _ in 0..6 {
        adapter.next_poll = Instant::now();
        results.push(adapter.poll().unwrap());
    }
    let requests = stop(port, server);
    let shows: Vec<&String> = requests
        .iter()
        .filter(|request| request.starts_with("POST /api/show"))
        .collect();
    assert_eq!(
        shows.len(),
        2,
        "one per digest, never per poll: {requests:?}"
    );
    assert_eq!(
        shows[0].as_str(),
        r#"POST /api/show {"model":"llama3:8b","runner":"llamacpp"}"#
    );
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.starts_with("GET /api/ps"))
            .count(),
        6
    );
    // The facts reach the first poll's result, and every one after.
    for result in &results {
        assert_eq!(result.model_facts.parameters, Some(8_030_261_248));
        assert_eq!(result.model_facts.context_trained, Some(8192));
    }
    // The diagnostics report lists only what the adapter asked for.
    assert!(adapter
        .report()
        .probes
        .iter()
        .all(|probe| probe.path == "/api/ps"));
}

#[test]
fn ollama_show_failures_leave_facts_unknown_and_are_not_asked_every_poll() {
    let (port, server) = recording(|method, path| match (method, path) {
        ("GET", "/api/ps") => (200, OLLAMA_PS.replace("2ae6f6dd", "feedface")),
        _ => (500, "{}".into()),
    });
    let mut failing = adapter("ollama", port);
    for _ in 0..3 {
        failing.next_poll = Instant::now();
        let result = failing.poll().unwrap();
        assert_eq!(result.model_facts.parameters, None);
        assert_eq!(result.model_facts.quantization.as_deref(), Some("Q4_0"));
    }
    let requests = stop(port, server);
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("POST /api/show"))
            .count(),
        1,
        "{requests:?}"
    );
    // An answer without `model_info` is an answer: kept, not asked again.
    let (port, server) = recording(|method, path| match (method, path) {
        ("GET", "/api/ps") => (200, OLLAMA_PS.replace("2ae6f6dd", "decade00")),
        _ => (200, r#"{"details":{"format":"gguf"}}"#.into()),
    });
    let mut quiet = adapter("ollama", port);
    for _ in 0..3 {
        quiet.next_poll = Instant::now();
        assert_eq!(quiet.poll().unwrap().model_facts.parameters, None);
    }
    let requests = stop(port, server);
    let shows = requests.iter().filter(|r| r.starts_with("POST /api/show"));
    assert_eq!(shows.count(), 1, "{requests:?}");
}

#[test]
fn never_asked_matches_each_route_its_subpaths_and_queries_only() {
    for path in [
        "/health_generate",
        "/flush_cache",
        "/flush_cache?now=1",
        "/unload",
        "/unload/llama",
        "/upstream/llama/v1/models",
        "/api/extra/shutdown",
        "/openai/load/phi-4",
        "/openai/unload/phi-4",
        "/openai/unloadall",
        "/openai/setgpudevice/1",
        "/bench/chat/completions",
        "/release_memory_occupation",
    ] {
        assert!(never_asked(path), "{path}");
    }
    for path in [
        "/health",
        "/metrics",
        "/api/ps",
        "/api/show",
        "/unloaded",
        "/upstream",
        "/v1/models",
        "/metrics?format=prometheus",
    ] {
        assert!(!never_asked(path), "{path}");
    }
    // Refused before any connection is tried.
    assert_eq!(
        Endpoint::default().get(1, "/flush_cache"),
        Err(ProbeIssue::InvalidEndpoint)
    );
}

#[test]
fn no_adapter_asks_a_route_that_changes_state_or_generates() {
    // Every adapter, polled against a server that answers like its runtime.
    let metrics = |provider: &str| match provider {
        "vllm" => VLLM_0.to_owned(),
        "sglang" => SGLANG_0.to_owned(),
        "tensorfold" => TENSORFOLD_0.to_owned(),
        "mlx-serve" => "vllm:num_requests_running 1\nmlx_serve:generation_tokens_live 10\n".into(),
        "llama.cpp" => "llamacpp:requests_processing 0\n".into(),
        _ => "vllm:num_requests_running 1\n".into(),
    };
    for provider in [
        "ollama",
        "lmstudio",
        "vllm",
        "sglang",
        "tensorfold",
        "strata",
        "mlx-serve",
        "splash",
        "llama.cpp",
        "koboldcpp",
        "mlx-lm",
        "localai",
        "jan",
        "gpt4all",
        "magnitude",
    ] {
        let body = metrics(provider);
        let (port, server) = recording(move |_, path| match path {
            "/metrics" | "/metrics?format=prometheus" => (200, body.clone()),
            "/metrics.json" => (200, r#"{"sessions":[]}"#.into()),
            "/api/ps" => (200, OLLAMA_PS.into()),
            "/api/show" => (200, OLLAMA_SHOW.into()),
            "/api/v1/models" => (200, LM_STUDIO_MODELS.into()),
            "/v1/models" | "/inference/v1/models" => (200, r#"{"data":[{"id":"m"}]}"#.into()),
            "/slots" => (200, "[]".into()),
            "/api/extra/perf" => (200, r#"{"total_gens":1}"#.into()),
            "/status" | "/health" => (200, "{}".into()),
            _ => (404, "{}".into()),
        });
        let mut adapter = adapter(provider, port);
        for _ in 0..3 {
            adapter.next_poll = Instant::now();
            adapter.poll();
        }
        let requests = stop(port, server);
        assert!(!requests.is_empty(), "{provider} asked nothing");
        for request in &requests {
            let path = request.split_whitespace().nth(1).unwrap_or_default();
            assert!(!never_asked(path), "{provider} asked {request}");
            // SGLang's /health runs a one-token generation by default.
            if provider == "sglang" {
                assert!(!path.starts_with("/health"), "{provider} asked {request}");
            }
        }
    }
}

#[test]
fn no_production_source_names_a_route_that_changes_state() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0;
    for entry in fs::read_dir(&source).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let mut text = fs::read_to_string(&path).unwrap();
        // The list of the routes itself.
        if let Some(start) = text.find("pub(super) const NEVER_ASKED") {
            let end = start + text[start..].find("];").unwrap();
            text.replace_range(start..end, "");
        }
        for route in NEVER_ASKED {
            assert!(
                !text.contains(route.trim_end_matches('/')),
                "{} names {route}",
                path.display()
            );
        }
        checked += 1;
    }
    assert!(checked > 40, "{checked} sources");
}

#[test]
fn ollama_show_carries_the_endpoints_key_and_the_model_in_a_json_body() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let ps = OLLAMA_PS.replace("2ae6f6dd", "c0ffee00");
    let digest = serde_json::from_str::<Value>(&ps).unwrap()["models"][0]["digest"]
        .as_str()
        .unwrap()
        .to_owned();
    let server = thread::spawn(move || {
        let mut heads = Vec::new();
        for reply in [ps.as_str(), OLLAMA_SHOW] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut head = Vec::new();
            let mut byte = [0; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            let head = String::from_utf8_lossy(&head).to_ascii_lowercase();
            let length: usize = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:")?.trim().parse().ok())
                .unwrap_or(0);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            heads.push((head, String::from_utf8(body).unwrap()));
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            )
            .unwrap();
        }
        heads
    });
    let endpoint = Endpoint {
        url: None,
        api_key: Some("test-token".into()),
        allow_remote_auth: false,
    };
    assert!(endpoint.get(port, "/api/ps").is_ok());
    let heads = server.join().unwrap();
    let (show, body) = &heads[1];
    assert!(show.starts_with("post /api/show http/1.1"), "{show}");
    assert!(show.contains("authorization: bearer test-token"), "{show}");
    assert!(show.contains("content-type: application/json"), "{show}");
    assert_eq!(body, r#"{"model":"llama3:8b","runner":"llamacpp"}"#);
    let known = OLLAMA_SHOWN.with_borrow(|shown| shown.facts(&digest));
    assert_eq!(
        known.and_then(|facts| facts.parameters),
        Some(8_030_261_248)
    );
}

#[test]
fn a_server_on_an_address_this_machine_holds_is_local_but_still_gets_no_credentials() {
    let held =
        held_interface_address().expect("a route off this machine, to find an address it holds");
    let host = match held {
        std::net::IpAddr::V4(ip) => ip.to_string(),
        std::net::IpAddr::V6(ip) => format!("[{ip}]"),
    };
    let endpoint = |api_key: Option<&str>| Endpoint {
        url: Some(format!("http://{host}:8080")),
        api_key: api_key.map(Into::into),
        allow_remote_auth: false,
    };
    assert!(!endpoint(None).is_remote());
    // The credentials rule stays loopback only: the key needs the opt-in
    // even though the server is on this machine.
    assert_eq!(
        endpoint(Some("test-token")).get(1, "/models"),
        Err(ProbeIssue::RemoteAuthBlocked)
    );
}
