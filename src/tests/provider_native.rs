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
    assert!(result.details.unwrap().contains("capacity 8192"));
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
        {"type":"embedding", "loaded_instances":[{"id":"embedding"}]}
    ]}))
    .unwrap();
    assert_eq!(lm.model.as_deref(), Some("instance"));
    assert_eq!(lm.model_memory, None);
    assert!(lm.details.unwrap().contains("4096"));
    assert!(lm_studio(&json!({"models":[{}]})).is_none());
    assert!(lm_studio(&json!({"models":[{"loaded_instances":[{}]}]})).is_none());
    let legacy = lm_studio_legacy(
        &json!({"data":[{"id":"loaded", "state":"loaded"}, {"state":"not-loaded"}]}),
    )
    .unwrap();
    assert_eq!(legacy.model.as_deref(), Some("loaded"));
    assert!(lm_studio_legacy(&json!({"data":[{}]})).is_none());
    for provider in ["Jan", "GPT4All", "mlx-lm", "LocalAI"] {
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
            "{}process_start_time_seconds {started}\n",
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
    assert!(second.generation_tps_live && second.prefill_tps_live);
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
        ("http://192.168.50.132:8000", true),
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
    let listener = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = Endpoint {
        url: Some(format!(
            "http://127.0.0.2:{}",
            listener.local_addr().unwrap().port()
        )),
        api_key: Some("test-token".into()),
        allow_remote_auth: false,
    };
    assert!(endpoint.get(1, "/models").is_err());
    assert!(listener.accept().is_err(), "no request or credentials sent");
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
