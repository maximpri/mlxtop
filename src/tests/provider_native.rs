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
fn a_prefill_that_ends_between_polls_shows_briefly_as_a_last_sample_and_then_reads_zero() {
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
    assert_eq!(poll(7, 500, 0.15), (Some(0.0), true), "gone after the hold");
}

#[test]
fn an_idle_server_reads_a_live_zero_after_a_long_gap_too() {
    let now = Instant::now();
    let mut history = MetricsHistory::default();
    for second in 0..3_u64 {
        let text = mlx_serve_prefill(0, false, 100, 1.0);
        let result = history
            .observe("mlx-serve", &text, now + Duration::from_secs(second))
            .unwrap();
        let expected = if second == 0 {
            (None, false)
        } else {
            (Some(0.0), true)
        };
        assert_eq!((result.prefill_tps, result.prefill_tps_live), expected);
    }
    let text = mlx_serve_prefill(0, false, 100, 1.0);
    let later = history
        .observe("mlx-serve", &text, now + Duration::from_secs(60))
        .unwrap();
    assert_eq!(
        (later.prefill_tps, later.prefill_tps_live),
        (Some(0.0), true)
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
    assert_eq!(
        result.prompt_tokens, None,
        "two requests: no single-request totals"
    );
    let mut one = LlmTelemetry::default();
    apply_mlx_serve_sessions(
        &mut one,
        &json!({"sessions":[{"phase":"decode","context_tokens":50,"generated_tokens":20}]}),
        now,
    );
    assert_eq!((one.prompt_tokens, one.output_tokens), (Some(30), Some(20)));
    let mut none = LlmTelemetry::default();
    apply_mlx_serve_sessions(&mut none, &json!({}), now);
    assert!(none.requests.is_empty() && none.model.is_none());
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
fn mlx_serve_prefill_reading_does_not_reappear_when_a_long_decode_ends() {
    let (now, mut h) = (Instant::now(), MetricsHistory::default());
    for (t, live) in [(0, 0), (1, 8192), (2, 16384)] {
        poll_at(&mut h, now, t, &mlx_serve_prefill(live, true, 0, 0.0));
    }
    for t in 3..60 {
        poll_at(&mut h, now, t, &mlx_serve_prefill(0, false, 0, 0.0));
    }
    let (rate, live) = poll_at(&mut h, now, 60, &mlx_serve_prefill(0, false, 16384, 2.0));
    assert!(
        live || rate.is_none(),
        "a stale prefill speed shows at decode end: {rate:?}"
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
        apply_mlx_serve_sessions(&mut r, &json!({ "sessions": sessions }), now);
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
