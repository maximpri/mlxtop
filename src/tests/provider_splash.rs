use crate::test_support::*;
// SPDX-License-Identifier: MIT
use super::*;

/// A `/status` as Splash's server answers it, with the counters a test moves.
fn status(decoded: u64, prefilled: u64, prefill_ms: f64, requests: Value) -> Value {
    json!({
        "schema_version": 1,
        "ready": true,
        "maximum_context_tokens": 131072,
        "memory_pressure": "normal",
        "thermal_state": "nominal",
        "requests": {"submitted": 3, "completed": 1, "cancelled": 0, "failed": 0},
        "active_requests": requests,
        "memory_actual": {"current_bytes": 20_000_000_000_u64, "peak_bytes": 22_000_000_000_u64},
        "memory_governor": {"limit_bytes": 30_000_000_000_u64, "headroom_bytes": 1},
        "weights": {"idle_release_seconds": 600, "released": false, "restoring": false},
        "cache": {"hits": 1, "cold_misses": 1, "reused_tokens": 3000},
        "metrics": {
            "ttft_ms": {"p50": 282.4, "p95": 900.0},
            "prefill_input_tokens": prefilled,
            "prefill_wall_ms": prefill_ms,
            "decode_output_tokens": decoded,
            "drafted_tokens": 400,
            "accepted_draft_tokens": 300,
            "draft_acceptance_rate": 0.75
        },
        "instance": {"id": "abc123", "pid": 42, "model": "unsloth/Qwen3.8-27B-GGUF:UD-Q4_K_M",
                     "host": "127.0.0.1", "port": 8000},
        "transport": {"ready": true, "recovering": false, "stopped": false, "restarts": 0,
                      "status_stale": false, "status_age_ms": 0.0}
    })
}

fn request(id: u64, phase: &str, prompt: u64, processed: u64, generated: u64) -> Value {
    json!({"id": id, "phase": phase, "priority": 0, "prompt_tokens": prompt,
           "prompt_processed": processed, "generated_tokens": generated,
           "max_new_tokens": 4096, "age_ms": 1500})
}

#[test]
fn reads_model_requests_memory_and_cache_from_status() {
    let mut history = SplashHistory::default();
    let requests = json!([
        request(7, "decode", 1200, 1200, 40),
        request(8, "prefill", 8000, 2000, 0),
        request(9, "queued", 500, 0, 0),
    ]);
    let result = history
        .observe(&status(1000, 6000, 3000.0, requests), Instant::now())
        .unwrap();
    assert_eq!(result.provider.as_deref(), Some("Splash"));
    assert_eq!(
        result.model.as_deref(),
        Some("unsloth/Qwen3.8-27B-GGUF:UD-Q4_K_M")
    );
    assert_eq!(result.status.as_deref(), Some("prefilling"));
    assert_eq!(result.active_requests, Some(2));
    assert_eq!(result.waiting_requests, Some(1));
    assert_eq!(result.requests.len(), 2, "queued requests are not running");
    assert_eq!(result.requests[0].id, "7");
    assert_eq!(result.requests[0].prompt, 1200);
    assert_eq!(result.requests[0].output, Some(40));
    // Decoding has no prefill progress; the prefilling request has 2000 of 8000.
    assert_eq!(result.requests[0].prefilled, None);
    assert_eq!(result.requests[1].prefilled, Some(2000));
    assert_eq!(
        result.prompt_tokens, None,
        "two requests have no single count"
    );
    assert_eq!(result.model_memory, Some(20_000_000_000));
    assert_eq!(result.model_memory_max, Some(30_000_000_000));
    // 3000 reused of 3000 + 6000 prompt tokens.
    assert_eq!(result.total_prompt_tokens, Some(9000));
    assert_eq!(result.total_cached_tokens, Some(3000));
    assert!((result.prefix_hit_rate.unwrap() - 100.0 / 3.0).abs() < 1e-9);
    let details = result.details.unwrap();
    for part in [
        "prefill 2000 / 8000 tokens (25%)",
        "context capacity 131072 tokens",
        "DFlash draft acceptance 75%",
        "server TTFT p50 282 ms",
    ] {
        assert!(details.contains(part), "{details}");
    }
    assert!(!details.contains("thermal") && !details.contains("pressure"));
    // The first capture has no rate yet.
    assert_eq!(result.generation_tps, None);
    assert_eq!(result.prefill_tps, None);
}

#[test]
fn rates_come_from_counter_growth_and_prefill_gpu_time() {
    let mut history = SplashHistory::default();
    let now = Instant::now();
    let decoding = json!([request(1, "decode", 100, 100, 10)]);
    history
        .observe(&status(1000, 6000, 3000.0, decoding.clone()), now)
        .unwrap();
    // 150 tokens decoded in 2 s; 4000 tokens prefilled in 1000 ms of prefill commands.
    let later = now + Duration::from_secs(2);
    let result = history
        .observe(&status(1150, 10000, 4000.0, decoding.clone()), later)
        .unwrap();
    assert_eq!(result.generation_tps, Some(75.0));
    assert!(result.generation_tps_live);
    assert_eq!(result.prefill_tps, Some(4000.0));
    assert!(
        !result.prefill_tps_live,
        "nothing is prefilling: a held reading"
    );
    assert_eq!(result.prompt_tokens, Some(100));
    assert_eq!(result.output_tokens, Some(10));
    // The held prefill speed gives way to a live 0.
    let result = history
        .observe(
            &status(1150, 10000, 4000.0, decoding),
            later + Duration::from_secs(4),
        )
        .unwrap();
    assert_eq!(result.generation_tps, Some(0.0));
    assert_eq!(result.prefill_tps, Some(0.0));
    assert!(result.prefill_tps_live);
}

#[test]
fn prefill_speed_holds_between_chunks_and_waits_for_the_first() {
    let mut history = SplashHistory::default();
    let now = Instant::now();
    let at = |s: u64| now + Duration::from_secs(s);
    let prefilling = |done| json!([request(1, "prefill", 32000, done, 0)]);
    history
        .observe(&status(0, 0, 0.0, json!([])), at(0))
        .unwrap();
    let result = history
        .observe(&status(0, 0, 0.0, prefilling(0)), at(1))
        .unwrap();
    assert_eq!(result.prefill_tps, None, "no chunk has finished");
    let result = history
        .observe(&status(0, 2048, 5000.0, prefilling(2048)), at(6))
        .unwrap();
    assert!((result.prefill_tps.unwrap() - 409.6).abs() < 1e-9);
    assert!(result.prefill_tps_live);
    // No chunk finished in this interval: still the last chunk's speed, live.
    let result = history
        .observe(&status(0, 2048, 5000.0, prefilling(2048)), at(7))
        .unwrap();
    assert!((result.prefill_tps.unwrap() - 409.6).abs() < 1e-9);
    assert!(result.prefill_tps_live);
}

#[test]
fn stale_snapshots_are_dated_by_their_age_and_restarts_reset_rates() {
    let mut history = SplashHistory::default();
    let now = Instant::now();
    history
        .observe(&status(1000, 0, 0.0, json!([])), now)
        .unwrap();
    let first = history
        .observe(
            &status(1100, 0, 0.0, json!([])),
            now + Duration::from_secs(1),
        )
        .unwrap();
    assert_eq!(first.generation_tps, Some(100.0));
    // A busy loop answers with the same snapshot, now 1.5 s old: no new capture, same rate.
    let mut stale = status(1100, 0, 0.0, json!([]));
    stale["transport"]["status_stale"] = json!(true);
    stale["transport"]["status_age_ms"] = json!(1500.0);
    let result = history
        .observe(&stale, now + Duration::from_millis(2500))
        .unwrap();
    assert_eq!(result.generation_tps, Some(100.0));
    // The engine restarted: counters began again, so no rate until the next capture.
    let mut restarted = status(5, 0, 0.0, json!([]));
    restarted["transport"]["restarts"] = json!(1);
    let result = history
        .observe(&restarted, now + Duration::from_secs(3))
        .unwrap();
    assert_eq!(result.generation_tps, None);
    let result = history
        .observe(&restarted, now + Duration::from_secs(4))
        .unwrap();
    assert_eq!(result.generation_tps, Some(0.0));
}

#[test]
fn engine_states_name_restarts_loading_pressure_and_released_weights() {
    let mut history = SplashHistory::default();
    let now = Instant::now();
    let with = |edit: &dyn Fn(&mut Value)| {
        let mut value = status(0, 0, 0.0, json!([]));
        edit(&mut value);
        value
    };
    let state = |history: &mut SplashHistory, value: Value| {
        let result = history.observe(&value, now).unwrap();
        (result.status.unwrap(), result.details.unwrap_or_default())
    };
    assert_eq!(state(&mut history, with(&|_| {})).0, "idle");
    let (idle, details) = state(
        &mut history,
        with(&|v| v["weights"]["released"] = json!(true)),
    );
    assert_eq!(idle, "idle");
    assert!(details.contains("weights released while idle"));
    let restoring = with(&|v| {
        v["weights"]["released"] = json!(true);
        v["weights"]["restoring"] = json!(true);
    });
    assert_eq!(state(&mut history, restoring).0, "loading");
    assert_eq!(
        state(&mut history, with(&|v| v["ready"] = json!(false))).0,
        "loading"
    );
    let (paused, details) = state(
        &mut history,
        with(&|v| {
            v["ready"] = json!(false);
            v["memory_pressure"] = json!("critical");
            v["thermal_state"] = json!("serious");
        }),
    );
    assert_eq!(paused, "paused");
    assert!(details.contains("memory pressure critical"));
    assert!(details.contains("thermal state serious"));
    assert_eq!(
        state(
            &mut history,
            with(&|v| v["transport"]["recovering"] = json!(true))
        )
        .0,
        "restarting"
    );
    assert_eq!(
        state(
            &mut history,
            with(&|v| v["transport"]["stopped"] = json!(true))
        )
        .0,
        "error"
    );
    let waiting =
        with(&|v| v["active_requests"] = json!([request(1, "waiting_resources", 9, 0, 0)]));
    assert_eq!(state(&mut history, waiting).0, "waiting");
}

#[test]
fn an_engine_that_has_not_started_still_reports_and_other_json_does_not() {
    let mut history = SplashHistory::default();
    let result = history
        .observe(
            &json!({"schema_version": 1, "ready": false}),
            Instant::now(),
        )
        .unwrap();
    assert_eq!(result.status.as_deref(), Some("loading"));
    assert_eq!(result.model.as_deref(), Some("none"));
    assert_eq!(result.active_requests, None);
    assert_eq!(result.generation_tps, None);
    assert!(history
        .observe(&json!({"data": []}), Instant::now())
        .is_none());
    assert!(history
        .observe(&json!({"ready": "yes"}), Instant::now())
        .is_none());
}

/// `status` with the engine's decode time, which per-request speeds read.
fn timed(decoded: u64, decode_ms: f64, requests: Value) -> Value {
    let mut value = status(decoded, 6000, 3000.0, requests);
    value["metrics"]["decode_wall_ms"] = json!(decode_ms);
    value
}

#[test]
fn a_request_decoding_alone_gets_its_own_speed_and_an_exact_final_count() {
    let mut history = SplashHistory::default();
    let now = Instant::now();
    let at = |seconds| now + Duration::from_secs(seconds);
    // Prefilling: no output speed yet.
    let result = history
        .observe(
            &timed(500, 10_000.0, json!([request(1, "prefill", 900, 400, 0)])),
            at(0),
        )
        .unwrap();
    assert_eq!(result.requests[0].output_tps, None);
    // First seen decoding: the engine's counters mark where its decoding began.
    let result = history
        .observe(
            &timed(510, 10_200.0, json!([request(1, "decode", 900, 900, 10)])),
            at(1),
        )
        .unwrap();
    assert_eq!(result.requests[0].output_tps, None);
    // 120 tokens in 2000 ms of decode time.
    let result = history
        .observe(
            &timed(630, 12_200.0, json!([request(1, "decode", 900, 900, 130)])),
            at(2),
        )
        .unwrap();
    assert_eq!(result.requests[0].output_tps, Some(60.0));
    assert!(!result.requests[0].completed);
    // It finished between captures: 40 more tokens than last seen, and 160 tokens in the
    // 2500 ms of decode time since it was first seen decoding.
    let result = history
        .observe(&timed(670, 12_700.0, json!([])), at(3))
        .unwrap();
    assert_eq!(result.requests.len(), 1);
    let done = &result.requests[0];
    assert!(done.completed);
    assert_eq!(done.id, "1");
    assert_eq!(done.output, Some(170));
    assert_eq!(done.output_tps, Some(64.0));
    assert_eq!(
        result.prompt_tokens, None,
        "a finished request is not the one running"
    );
    // Reported once.
    let result = history
        .observe(&timed(670, 12_700.0, json!([])), at(4))
        .unwrap();
    assert!(result.requests.is_empty());
}

#[test]
fn concurrent_requests_measure_their_own_token_growth() {
    let mut history = SplashHistory::default();
    let now = Instant::now();
    let at = |seconds| now + Duration::from_secs(seconds);
    let both = |a, b| {
        json!([
            request(1, "decode", 900, 900, a),
            request(2, "decode", 300, 300, b)
        ])
    };
    history
        .observe(&timed(100, 1000.0, both(10, 5)), at(0))
        .unwrap();
    let result = history
        .observe(&timed(300, 3000.0, both(110, 105)), at(1))
        .unwrap();
    assert!(result.requests.iter().all(|r| r.output_tps == Some(100.0)));
    // Request 2 finishes while 1 runs on: preserve its last measured speed/count.
    let result = history
        .observe(
            &timed(400, 4000.0, json!([request(1, "decode", 900, 900, 210)])),
            at(2),
        )
        .unwrap();
    let done: Vec<_> = result.requests.iter().filter(|r| r.completed).collect();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].id, "2");
    assert_eq!(done[0].output, Some(105));
    assert_eq!(done[0].output_tps, Some(100.0));
    // The remaining request still uses its own counts, not the shared GPU timer.
    let running = result.requests.iter().find(|r| !r.completed).unwrap();
    assert_eq!(running.output_tps, Some(100.0));
}

#[test]
fn a_restarted_engine_forgets_its_requests_without_finishing_them() {
    let mut history = SplashHistory::default();
    let now = Instant::now();
    history
        .observe(
            &timed(100, 1000.0, json!([request(1, "decode", 900, 900, 10)])),
            now,
        )
        .unwrap();
    let mut restarted = timed(0, 0.0, json!([]));
    restarted["transport"]["restarts"] = json!(1);
    let result = history
        .observe(&restarted, now + Duration::from_secs(1))
        .unwrap();
    assert!(result.requests.is_empty());
}

#[test]
fn details_lead_with_problems_then_latency_memory_and_capacity() {
    let mut value = status(1000, 6000, 3000.0, json!([]));
    value["requests"] = json!({"submitted": 412, "completed": 408, "cancelled": 1, "failed": 3});
    value["metal"] = json!({"healthy": false});
    value["metrics"]["metal_failures"] = json!(2);
    value["metrics"]["capacity_failures"] = json!(0);
    value["metrics"]["itl_ms"] = json!({"p50": 12.4, "p95": 31.0});
    value["admission"] = json!({"waiting_memory": 1});
    value["disk"] = json!({"writes_gated": "budget"});
    value["thermal_state"] = json!("serious");
    value["memory_governor"]["headroom_bytes"] = json!(4_400_000_000_u64);
    let details = SplashHistory::default()
        .observe(&value, Instant::now())
        .unwrap()
        .details
        .unwrap();
    let parts: Vec<&str> = details.split(" · ").collect();
    assert_eq!(
        &parts[..7],
        [
            "3 failed",
            "1 cancelled of 412 requests",
            "Metal unhealthy",
            "2 Metal failures",
            "1 waiting for memory",
            "SSD cache writes gated (budget)",
            "thermal state serious",
        ],
        "{details}"
    );
    for part in [
        "server TTFT p50 282 ms · p95 900 ms",
        "inter-token p50 12 ms · p95 31 ms",
        "Metal peak 20.5 GiB · headroom 4.1 GiB",
    ] {
        assert!(details.contains(part), "{details}");
    }
    assert!(
        !details.contains("capacity failures"),
        "zero is not a problem"
    );
}

#[test]
fn splash_release_fixtures_preserve_the_monitoring_contract() {
    for fixture in [
        include_str!("fixtures/splash/1.3.2.json"),
        include_str!("fixtures/splash/1.4.0.json"),
        include_str!("fixtures/splash/1.4.1.json"),
        include_str!("fixtures/splash/1.5.0.json"),
    ] {
        let value: Value = serde_json::from_str(fixture).unwrap();
        let now = Instant::now();
        let mut history = SplashHistory::default();
        let result = history.observe(&value, now).unwrap();
        assert_eq!(result.active_requests, Some(2));
        assert_eq!(result.status.as_deref(), Some("prefilling"));
        assert_eq!(result.model_memory, Some(4294967296));
        assert_eq!(result.requests.len(), 2);
        assert_eq!(result.requests[0].prompt, 4096);
        assert_eq!(result.requests[0].prefilled, Some(2048));
        assert_eq!(result.requests[1].output, Some(40));
        let result = history
            .observe(&value, now + Duration::from_secs(1))
            .unwrap();
        assert_eq!(result.generation_tps, Some(0.0));
    }
}

#[test]
fn concurrent_rates_do_not_share_tokens_or_survive_reused_request_ids() {
    let mut history = SplashHistory::default();
    let now = Instant::now();
    let both = |a, b| {
        json!([
            request(1, "decode", 100, 100, a),
            request(2, "decode", 200, 200, b)
        ])
    };
    history
        .observe(&status(0, 0, 0.0, both(10, 10)), now)
        .unwrap();
    let result = history
        .observe(
            &status(120, 0, 0.0, both(30, 110)),
            now + Duration::from_secs(2),
        )
        .unwrap();
    assert_eq!(result.requests[0].output_tps, Some(10.0));
    assert_eq!(result.requests[1].output_tps, Some(50.0));
    let result = history
        .observe(
            &status(130, 0, 0.0, json!([request(1, "decode", 300, 300, 1)])),
            now + Duration::from_secs(3),
        )
        .unwrap();
    assert_eq!(result.requests[0].output_tps, None);
}
