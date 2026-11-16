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
        "DFlash draft acceptance 75%",
        "server TTFT p50 282 ms",
    ] {
        assert!(details.contains(part), "{details}");
    }
    assert!(!details.contains("thermal") && !details.contains("pressure"));
    // The context Splash allows is a model fact, said once there.
    assert!(!details.contains("context"), "{details}");
    assert_eq!(result.model_facts.context_configured, Some(131_072));
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
fn concurrent_requests_share_counters_so_get_no_per_request_speed() {
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
    assert!(result.requests.iter().all(|r| r.output_tps.is_none()));
    // Request 2 finishes while 1 runs on: its last observed count, no speed.
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
    assert_eq!(done[0].output_tps, None);
    // Request 1 shared the engine before, so its speed stays unknown.
    let running = result.requests.iter().find(|r| !r.completed).unwrap();
    assert_eq!(running.output_tps, None);
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
fn draft_acceptance_is_measured_between_polls_not_over_the_engines_life() {
    let mut history = SplashHistory::default();
    let now = Instant::now();
    let decoding = json!([request(1, "decode", 100, 100, 10)]);
    let with_drafts = |drafted: u64, accepted: u64| {
        let mut value = status(1000 + drafted, 6000, 3000.0, decoding.clone());
        value["metrics"]["drafted_tokens"] = json!(drafted);
        value["metrics"]["accepted_draft_tokens"] = json!(accepted);
        value
    };
    let first = history.observe(&with_drafts(400, 300), now).unwrap();
    assert_eq!(first.draft_acceptance, None, "one poll has no interval");
    // 100 drafted since, 40 accepted: 40% now, though 68% over its life.
    let next = history
        .observe(&with_drafts(500, 340), now + Duration::from_secs(1))
        .unwrap();
    assert_eq!(next.draft_acceptance, Some(0.4));
    // Nothing drafted in the interval: no reading.
    let idle = history
        .observe(&with_drafts(500, 340), now + Duration::from_secs(2))
        .unwrap();
    assert_eq!(idle.draft_acceptance, None);
    // A model without a draft never reports one.
    let mut plain = status(2000, 6000, 3000.0, decoding.clone());
    plain["metrics"]
        .as_object_mut()
        .unwrap()
        .retain(|key, _| !key.contains("draft"));
    history.observe(&plain, now + Duration::from_secs(3));
    let later = history
        .observe(&plain, now + Duration::from_secs(4))
        .unwrap();
    assert_eq!(later.draft_acceptance, None);
}

#[test]
fn an_absurd_status_age_from_splash_is_ignored_not_fatal() {
    let mut history = SplashHistory::default();
    let mut absurd = status(1000, 0, 0.0, json!([]));
    absurd["transport"]["status_age_ms"] = json!(1e30);
    assert!(history.observe(&absurd, Instant::now()).is_some());
}

/// Splash 1.5.0's `/status` on the owner's Mac Studio, where oMLX 0.7.3 runs it: redacted,
/// its instance id replaced and its latency histograms left out.
const OWNER_STATUS: &str = include_str!("fixtures/splash_status.json");
/// That engine's counters every 2 s while six requests ran one after another, and the
/// Splash log's Done line for each.
const OWNER_COUNTERS: &str = include_str!("fixtures/splash_counters.json");

fn owner_status() -> Value {
    serde_json::from_str(OWNER_STATUS).unwrap()
}

/// The owner's status with one capture's counters.
fn owner_capture(capture: &Value) -> Value {
    let mut status = owner_status();
    for field in [
        "decode_output_tokens",
        "drafted_tokens",
        "accepted_draft_tokens",
    ] {
        status["metrics"][field] = capture[field].clone();
    }
    status["scheduler"]["decode_batches"] = capture["decode_batches"].clone();
    status["scheduler"]["decode_batches_by_width"] = capture["by_width"].clone();
    status
}

#[test]
fn the_owners_status_gives_kv_room_suspensions_and_the_models_facts() {
    let result = SplashHistory::default()
        .observe(&owner_status(), Instant::now())
        .unwrap();
    assert_eq!(result.status.as_deref(), Some("idle"));
    assert_eq!(
        result.model.as_deref(),
        Some("omlx/Qwen3.8-27B-oQ4e-mtp-e5133ef4")
    );
    // The plan's 4,736 KV pages of 32 tokens, none held by a running request: the 505
    // holding cached prefixes are room.
    assert_eq!(result.kv_free_bytes, Some(4_736 * 1_064_960));
    assert_eq!(result.kv_free_tokens, Some(151_552));
    assert_eq!(result.preemptions_total, Some(4));
    assert_eq!(result.tokens_per_pass, None, "one capture has no interval");
    let facts = result.model_facts;
    assert_eq!(
        (facts.kv_layers, facts.kv_heads, facts.head_dim),
        (Some(16), Some(4), Some(256))
    );
    assert_eq!(facts.kv_dtype.as_deref(), Some("int8"));
    // What Splash plans with is what the model's own files give: 33,280 bytes of int8 KV a
    // token, and a 152.6 MiB state per request.
    assert_eq!(facts.kv_bytes_per_token(), Some(33_280));
    assert_eq!(facts.fixed_state_bytes, Some(160_038_912));
    assert_eq!(
        (facts.context_configured, facts.context_trained),
        (Some(131_072), Some(262_144))
    );
    assert_eq!(facts.draft, Some(true));
    // It names neither what a pass reads nor the draft's size as stored.
    assert_eq!((facts.bytes_per_pass, facts.draft_bytes), (None, None));
    let details = result.details.unwrap();
    for part in [
        "46 cancelled of 9512 requests",
        "DFlash draft acceptance 65%",
        "server TTFT p50 2705 ms · p95 28949 ms",
    ] {
        assert!(details.contains(part), "{details}");
    }
    // Splash 1.5 counts no decode gaps or first-token parts.
    assert!(
        !details.contains("paused") && !details.contains("first token"),
        "{details}"
    );
}

#[test]
fn tokens_per_pass_from_the_owners_counters_match_the_splash_logs_tokens_per_cycle() {
    let fixture: Value = serde_json::from_str(OWNER_COUNTERS).unwrap();
    let captures = fixture["captures"].as_array().unwrap();
    let logged: Vec<f64> = fixture["done"]
        .as_array()
        .unwrap()
        .iter()
        .map(|done| done["tok_per_cycle"].as_f64().unwrap())
        .collect();
    let passes = |capture: &Value| -> u64 {
        capture["by_width"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(width, n)| width[1..].parse::<u64>().unwrap() * n.as_u64().unwrap())
            .sum()
    };
    let completed = |capture: &Value| capture["completed"].as_u64().unwrap();
    let start = Instant::now();
    let at = |capture: &Value| start + Duration::from_millis(capture["ms"].as_u64().unwrap());
    let first = completed(&captures[0]);
    // Each poll's reading lies near the request that ran in it; a poll with fewer than eight
    // passes gives none.
    let mut history = SplashHistory::default();
    let mut read = 0;
    for pair in captures.windows(2) {
        history.observe(&owner_capture(&pair[0]), at(&pair[0]));
        let result = history
            .observe(&owner_capture(&pair[1]), at(&pair[1]))
            .unwrap();
        let interval = passes(&pair[1]) - passes(&pair[0]);
        let request = (completed(&pair[0]) - first) as usize;
        match result.tokens_per_pass {
            Some(tokens) => {
                assert!(interval >= MIN_PASSES);
                let logged = logged[request];
                assert!(
                    (tokens / logged - 1.0).abs() < 0.06,
                    "request {request}: {tokens:.3} a pass against {logged} logged"
                );
                read += 1;
            }
            None => assert!(interval < MIN_PASSES, "{interval} passes unread"),
        }
    }
    assert!(read >= 12, "{read} polls read");
    // Over each whole request, from the capture before it to the one after it finished, the
    // counters give the log's figure to its last digit.
    for (request, logged) in logged.iter().enumerate() {
        let index = |n: u64| {
            captures
                .iter()
                .position(|capture| completed(capture) == first + n)
                .unwrap()
        };
        let (before, after) = (
            &captures[index(request as u64)],
            &captures[index(request as u64 + 1)],
        );
        let tokens = Counters::read(&owner_capture(after), at(after))
            .unwrap()
            .tokens_per_pass(&Counters::read(&owner_capture(before), at(before)).unwrap());
        if passes(after) - passes(before) < MIN_PASSES {
            // The 29-token answer took four passes: too few to say.
            assert_eq!(tokens, None);
        } else {
            let tokens = tokens.unwrap();
            assert!(
                (tokens - logged).abs() <= 0.05,
                "request {request}: {tokens:.3} against {logged}"
            );
        }
    }
    // Over the engine's life: 1 + 5,611,796 accepted over 1,227,137 passes. The log's 9,466
    // Done lines since that engine started give 6,824,523 tokens over 1,221,782 cycles, 5.586
    // (a request's first token comes from its prefill, so the log's figure is a little higher).
    let mut fresh = owner_status();
    for field in [
        "decode_output_tokens",
        "drafted_tokens",
        "accepted_draft_tokens",
    ] {
        fresh["metrics"][field] = json!(0);
    }
    fresh["scheduler"]["decode_batches_by_width"] = json!({"b1": 0, "b2": 0, "b3": 0, "b4": 0});
    let life = Counters::read(&owner_status(), start)
        .unwrap()
        .tokens_per_pass(&Counters::read(&fresh, start).unwrap())
        .unwrap();
    assert!((life / 5.586 - 1.0).abs() < 0.01, "{life}");
}

#[test]
fn tokens_per_pass_needs_a_draft_enough_passes_and_widths_it_knows() {
    let start = Instant::now();
    let at = |seconds| start + Duration::from_secs(seconds);
    let with = |decoded: u64, drafted: u64, accepted: u64, widths: Value| {
        let mut value = status(
            decoded,
            6000,
            3000.0,
            json!([request(1, "decode", 100, 100, 9)]),
        );
        value["metrics"]["drafted_tokens"] = json!(drafted);
        value["metrics"]["accepted_draft_tokens"] = json!(accepted);
        value["scheduler"] = json!({"decode_batches_by_width": widths});
        value
    };
    let mut history = SplashHistory::default();
    history.observe(&with(1000, 400, 300, json!({"b1": 100, "b2": 10})), at(0));
    // Eight batches alone and one beside another request: ten passes, 60 of 70 drafted
    // tokens accepted.
    let result = history
        .observe(&with(1070, 470, 360, json!({"b1": 108, "b2": 11})), at(1))
        .unwrap();
    assert_eq!(result.tokens_per_pass, Some(7.0));
    assert_eq!(result.draft_acceptance, Some(60.0 / 70.0));
    // A busy loop answering with the capture it gave: the same reading.
    let mut stale = with(1070, 470, 360, json!({"b1": 108, "b2": 11}));
    stale["transport"]["status_age_ms"] = json!(1000.0);
    let result = history.observe(&stale, at(2)).unwrap();
    assert_eq!(result.tokens_per_pass, Some(7.0));
    assert_eq!(result.draft_acceptance, Some(60.0 / 70.0));
    // Four passes say too little.
    let result = history
        .observe(&with(1100, 498, 386, json!({"b1": 112, "b2": 11})), at(3))
        .unwrap();
    assert_eq!(result.tokens_per_pass, None);
    // A width it does not know leaves the passes uncounted.
    history.observe(&with(1100, 498, 386, json!({"b1": 112, "wide": 1})), at(4));
    let result = history
        .observe(&with(1200, 598, 486, json!({"b1": 125, "wide": 1})), at(5))
        .unwrap();
    assert_eq!(result.tokens_per_pass, None);
    // Without a draft there are no draft counters.
    let mut plain = with(1300, 0, 0, json!({"b1": 125}));
    plain["metrics"]
        .as_object_mut()
        .unwrap()
        .retain(|key, _| !key.contains("draft"));
    history.observe(&plain, at(6));
    let mut later = plain.clone();
    later["metrics"]["decode_output_tokens"] = json!(1340);
    later["scheduler"] = json!({"decode_batches_by_width": {"b1": 165}});
    assert_eq!(
        history.observe(&later, at(7)).unwrap().tokens_per_pass,
        None
    );
    // A restarted engine's counters are not compared with the old one's.
    history.observe(&with(2000, 900, 700, json!({"b1": 200})), at(8));
    let mut restarted = with(80, 70, 60, json!({"b1": 10}));
    restarted["transport"]["restarts"] = json!(1);
    assert_eq!(
        history.observe(&restarted, at(9)).unwrap().tokens_per_pass,
        None
    );
}

#[test]
fn kv_room_is_the_plans_pages_less_those_running_requests_hold() {
    let room = |edit: &dyn Fn(&mut Value)| {
        let mut status = owner_status();
        edit(&mut status);
        let result = SplashHistory::default()
            .observe(&status, Instant::now())
            .unwrap();
        result.kv_free_bytes.zip(result.kv_free_tokens)
    };
    // A 63,680-token context holds 1,990 pages.
    assert_eq!(
        room(&|status| status["kv"]["pages_active"] = json!(1_990)),
        Some((2_746 * 1_064_960, 87_872))
    );
    // Sliding-window and spilling targets count their pages otherwise, a plan that did not
    // hold says nothing, and without the pages in use the room is unknown.
    let budget = |field: &'static str, value: Value| {
        move |status: &mut Value| status["memory_plan"]["budget"][field] = value.clone()
    };
    assert_eq!(
        room(&budget("kv_window", json!({"slim_page_bytes": 4096}))),
        None
    );
    assert_eq!(room(&budget("kv_spill", json!({"quota_bytes": 1}))), None);
    assert_eq!(
        room(&|status| status["memory_plan"]["valid"] = json!(false)),
        None
    );
    assert_eq!(
        room(&|status| status["kv"] = json!({"pages_allocated": 512})),
        None
    );
}

/// The owner's status with Splash 1.6's attribution, shaped as its api.md documents it (the
/// owner's Splash 1.5.0 has none): decode gaps by first cause, first-token parts in the shape
/// of `decode_gap_ms`, Metal pipelines and KV on the SSD.
fn holdups(
    prefill: (u64, f64),
    memory: (u64, f64),
    first: (u64, [f64; 5]),
    pipelines: u64,
    spill: (u64, u64, f64),
) -> Value {
    let mut status = owner_status();
    status["metrics"]["decode_gap_causes"] = json!({
        "prefill_unit": {"gaps": prefill.0, "ms": prefill.1, "prefill_ms": prefill.1 / 2.0},
        "claim_wait": {"gaps": memory.0, "ms": memory.1},
        "ssd_read": {"gaps": 0, "ms": 0.0},
        "pool_release": {"gaps": 0, "ms": 0.0},
        "replan": {"gaps": 0, "ms": 0.0},
        "other": {"gaps": 0, "ms": 0.0},
        "a_cause_not_documented": {"gaps": prefill.0, "ms": prefill.1}
    });
    let part = |total: f64| json!({"p50": 1.0, "p95": 2.0, "p99": 3.0, "max": 4.0, "total": total, "samples": first.0});
    let [own, claim, decode, other, rest] = first.1;
    status["metrics"]["ttft_parts_ms"] = json!({
        "own_prefill": part(own), "claim_wait": part(claim), "decode": part(decode),
        "other_prefill": part(other), "rest": part(rest),
        "a_part_not_documented": part(own)
    });
    status["metal"]["pipelines_created_while_serving"] = json!(pipelines);
    status["kv_spill"] = json!({
        "releases": spill.0, "released_pages": spill.0 * 4,
        "history_stream": {"bytes": spill.1, "read_ms": {"total": spill.2, "p50": 1.0}}
    });
    status["memory_plan"]["budget"]["kv_spill"] = json!({"quota_bytes": 1});
    status
}

#[test]
fn splash_1_6_says_what_held_decoding_and_first_tokens_up_between_polls() {
    let start = Instant::now();
    let mut history = SplashHistory::default();
    let gib = 1_u64 << 30;
    history.observe(
        &holdups(
            (10, 900.0),
            (2, 100.0),
            (100, [1e5, 0.0, 2e4, 3e5, 1e4]),
            5,
            (1, gib, 1000.0),
        ),
        start,
    );
    let busy = || {
        holdups(
            (13, 1320.0),
            (3, 180.0),
            (101, [101_200.0, 0.0, 20_010.0, 303_400.0, 10_500.0]),
            7,
            (3, gib + (600 << 20), 1500.0),
        )
    };
    let mut later = busy();
    later["memory_governor"]["claims"] = json!({"waiting": 1, "bytes": 2 * gib, "oldest_ms": 3200});
    let result = history
        .observe(&later, start + Duration::from_secs(1))
        .unwrap();
    assert_eq!((result.kv_free_bytes, result.kv_free_tokens), (None, None));
    let details = result.details.unwrap();
    let parts: Vec<&str> = details.split(" · ").collect();
    // Problems lead: a request waiting to grow, kernels compiled while serving.
    assert_eq!(
        &parts[..3],
        [
            "46 cancelled of 9512 requests",
            "1 waiting 3s for 2.0 GiB more memory",
            "2 Metal pipelines compiled while serving",
        ],
        "{details}"
    );
    for part in [
        "decode paused 3× for prefill (420 ms), 1× for memory (80 ms)",
        // The first token's 10 ms behind decoding is under a twentieth of its 5.1 s.
        "first token 5.1 s: 3.4 s behind another prefill, 1.2 s own prefill, 500 ms host and queue",
        "KV released to the SSD 2× for memory",
        "KV read from the SSD at 1.2 GiB/s",
    ] {
        assert!(details.contains(part), "{part:?} in {details}");
    }
    assert!(!details.contains("not_documented"), "{details}");
    // Nothing held anything up since: nothing to say.
    let quiet = history
        .observe(&busy(), start + Duration::from_secs(2))
        .unwrap()
        .details
        .unwrap();
    for word in ["more memory", "pipeline", "paused", "first token", "KV "] {
        assert!(!quiet.contains(word), "{word:?} in {quiet}");
    }
    // Several first tokens: what held them on average.
    let mut two = busy();
    for (part, total) in [("own_prefill", 103_200.0), ("other_prefill", 303_400.0)] {
        two["metrics"]["ttft_parts_ms"][part]["total"] = json!(total);
    }
    for part in [
        "own_prefill",
        "claim_wait",
        "decode",
        "other_prefill",
        "rest",
    ] {
        two["metrics"]["ttft_parts_ms"][part]["samples"] = json!(103);
    }
    let details = history
        .observe(&two, start + Duration::from_secs(3))
        .unwrap()
        .details
        .unwrap();
    assert!(
        details.contains("2 first tokens, 1.0 s each: 1.0 s own prefill"),
        "{details}"
    );
    // A first token that took no time has nothing to say.
    let mut instant = two.clone();
    for part in instant["metrics"]["ttft_parts_ms"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        part["samples"] = json!(104);
    }
    let details = history
        .observe(&instant, start + Duration::from_secs(4))
        .unwrap()
        .details
        .unwrap();
    assert!(!details.contains("first token"), "{details}");
}
