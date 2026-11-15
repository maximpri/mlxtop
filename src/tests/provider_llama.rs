use crate::test_support::*;
// SPDX-License-Identifier: MIT
// llama-server, read from the reply shapes of llama.cpp v1.0.0
// (2026-11-13), each fixture naming the function that writes it.
use super::*;
use std::cell::RefCell;
use std::collections::HashMap;

/// `GET /slots` from a server with four slots, as
/// server-context.cpp@v1.0.0 `server_slot::to_json(true)` writes it (the GET
/// handler's default: no prompt or generated text). Slot 0 answers a prompt
/// of 30,720 tokens, 24,576 of them from the cache, with 293 tokens out
/// (its context holds the prompt and all but the newest token); slot 1 reads
/// a prompt, 8,192 tokens from the cache and 2,048 computed with a batch in
/// flight; slot 2 idles after a task; slot 3 has run none. `next_token` is a
/// list, one entry per sequence; `params` is `task_params::to_json(true)`,
/// abridged, which mlxtop does not read.
fn slots_fixture() -> Value {
    let params = json!({
        "seed": 4294967295_u64, "temperature": 0.6, "top_k": 20, "top_p": 0.95,
        "max_tokens": -1, "n_predict": -1, "n_keep": 0, "n_discard": 0,
        "stream": true, "chat_format": "Hermes 2 Pro", "reasoning_format": "deepseek",
        "samplers": ["top_k", "top_p", "temperature"],
        "speculative.types": "draft", "timings_per_token": false, "lora": []
    });
    json!([
        {"id": 0, "n_ctx": 32768, "speculative": true, "is_processing": true,
         "id_task": 4211, "n_prompt_tokens": 31012, "n_prompt_tokens_processed": 6144,
         "n_prompt_tokens_cache": 24576, "params": params,
         "next_token": [{"has_next_token": true, "has_new_line": false,
                         "n_remain": -1, "n_decoded": 293}]},
        {"id": 1, "n_ctx": 32768, "speculative": true, "is_processing": true,
         "id_task": 4215, "n_prompt_tokens": 12288, "n_prompt_tokens_processed": 2048,
         "n_prompt_tokens_cache": 8192, "params": params,
         "next_token": [{"has_next_token": true, "has_new_line": false,
                         "n_remain": -1, "n_decoded": 0}]},
        {"id": 2, "n_ctx": 32768, "speculative": true, "is_processing": false,
         "id_task": 4100, "n_prompt_tokens": 5120, "n_prompt_tokens_processed": 512,
         "n_prompt_tokens_cache": 4096, "params": params,
         "next_token": [{"has_next_token": false, "has_new_line": true,
                         "n_remain": -1, "n_decoded": 513}]},
        {"id": 3, "n_ctx": 32768, "speculative": true, "is_processing": false}
    ])
}

/// The same server with every slot idle.
fn idle_slots() -> Value {
    let mut slots = slots_fixture();
    for slot in slots.as_array_mut().unwrap() {
        slot["is_processing"] = json!(false);
    }
    slots
}

/// A number as `std::ostream` prints a double by default (`%g`, six
/// significant digits), which is how llama-server prints every value.
/// Checked against a C++ build in `doubles_print_as_llama_server_prints_them`.
fn g6(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    let exponent = value.abs().log10().floor() as i32;
    if !(-5..6).contains(&exponent) {
        let mantissa = format!("{:.5}", value / 10_f64.powi(exponent));
        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
        let sign = if exponent < 0 { '-' } else { '+' };
        return format!("{mantissa}e{sign}{:02}", exponent.abs());
    }
    let decimals = (5 - exponent).max(0) as usize;
    let fixed = format!("{value:.decimals$}");
    if fixed.contains('.') {
        fixed.trim_end_matches('0').trim_end_matches('.').into()
    } else {
        fixed
    }
}

/// One scrape's numbers.
#[derive(Clone, Copy)]
struct Scrape {
    prompt: f64,
    cached: f64,
    prompt_seconds: f64,
    predicted: f64,
    predicted_seconds: f64,
    decodes: f64,
    drafted: f64,
    accepted: f64,
    passes: f64,
    processing: u64,
    deferred: u64,
    busy_per_decode: f64,
}

impl Default for Scrape {
    fn default() -> Self {
        Self {
            prompt: 18432.0,
            cached: 61440.0,
            prompt_seconds: 23.04,
            predicted: 4096.0,
            predicted_seconds: 102.4,
            decodes: 1500.0,
            drafted: 4800.0,
            accepted: 2880.0,
            passes: 1200.0,
            processing: 0,
            deferred: 0,
            busy_per_decode: 1.6,
        }
    }
}

/// `GET /metrics` as server-task.cpp@v1.0.0
/// `server_task_result_metrics::to_metrics` prints it: each counter, then
/// each gauge, with its HELP and TYPE lines, then the tokens accepted at
/// each draft position.
fn metrics_text(s: &Scrape) -> String {
    let counters = [
        (
            "prompt_tokens_total",
            "Number of prompt tokens processed, excluding cached tokens",
            s.prompt,
        ),
        (
            "prompt_tokens_cached_total",
            "Number of prompt tokens reused from the cache",
            s.cached,
        ),
        (
            "prompt_seconds_total",
            "Total time spent processing prompts",
            s.prompt_seconds,
        ),
        (
            "tokens_predicted_total",
            "Number of generation tokens processed",
            s.predicted,
        ),
        (
            "tokens_predicted_seconds_total",
            "Total time spent generating tokens",
            s.predicted_seconds,
        ),
        (
            "n_decode_total",
            "Total number of llama_decode() calls, excluding speculative decoding and multimodal decoding",
            s.decodes,
        ),
        (
            "n_tokens_max",
            "Largest observed sequence length (prompt + generation)",
            31012.0,
        ),
        (
            "spec_decode_num_draft_tokens_total",
            "Speculative: Total draft tokens generated",
            s.drafted,
        ),
        (
            "spec_decode_num_accepted_tokens_total",
            "Speculative: Total draft tokens accepted by the target model",
            s.accepted,
        ),
        (
            "spec_decode_num_drafts_total",
            "Speculative: Total speculative decoding verification steps",
            s.passes,
        ),
    ];
    let gauges = [
        (
            "prompt_tokens_seconds",
            "Average prompt throughput in tokens/s",
            0.0,
        ),
        (
            "predicted_tokens_seconds",
            "Average generation throughput in tokens/s",
            0.0,
        ),
        (
            "requests_processing",
            "Number of requests processing",
            s.processing as f64,
        ),
        (
            "requests_deferred",
            "Number of requests deferred",
            s.deferred as f64,
        ),
        (
            "n_busy_slots_per_decode",
            "Average number of busy slots per llama_decode() call",
            s.busy_per_decode,
        ),
    ];
    let mut text = String::new();
    for (kind, items) in [("counter", &counters[..]), ("gauge", &gauges[..])] {
        for (name, help, value) in items {
            text.push_str(&format!(
                "# HELP llamacpp:{name} {help}\n# TYPE llamacpp:{name} {kind}\nllamacpp:{name} {}\n",
                g6(*value)
            ));
        }
    }
    text.push_str("# HELP llamacpp:spec_decode_num_accepted_tokens_per_pos_total Accepted tokens per draft position\n# TYPE llamacpp:spec_decode_num_accepted_tokens_per_pos_total counter\n");
    for (position, accepted) in [1100, 850, 560, 370].into_iter().enumerate() {
        text.push_str(&format!(
            "llamacpp:spec_decode_num_accepted_tokens_per_pos_total{{position=\"{position}\"}} {accepted}\n"
        ));
    }
    text
}

/// `GET /props` as server-context.cpp@v1.0.0 `get_res_props` writes it,
/// awake or, from the reply cached as it fell asleep, sleeping. The chat
/// template is abridged; mlxtop reads neither it nor the model's path.
fn props_fixture(sleeping: bool) -> Value {
    json!({
        "default_generation_settings": {
            "params": {"seed": 4294967295_u64, "temperature": 0.8, "top_k": 40,
                       "speculative.types": "none", "lora": []},
            "n_ctx": 32768
        },
        "total_slots": 4,
        "model_alias": "/models/Qwen3-8B-Q4_K_M.gguf",
        "model_ftype": "Q4_K - Medium",
        "model_path": "/models/Qwen3-8B-Q4_K_M.gguf",
        "modalities": {"vision": false, "video": false, "audio": false},
        "media_marker": "<__media_YoNhud46VdDqbuFmKYEO9PY7A4ARzRfg__>",
        "endpoint_slots": true,
        "endpoint_props": false,
        "endpoint_metrics": true,
        "ui": true,
        "ui_settings": {},
        "chat_template": "{%- for message in messages %}{{- '<|im_start|>' + message.role }}{%- endfor %}",
        "chat_template_caps": {"supports_tools": true},
        "bos_token": "",
        "eos_token": "<|im_end|>",
        "build_info": "b8240-4f1e2a7",
        "is_sleeping": sleeping,
        "cors_proxy_enabled": false
    })
}

/// `GET /v1/models` as server-context.cpp@v1.0.0 `get_res_models` writes
/// it: an Ollama-style list beside OpenAI's `data`, whose `meta` README@v1.0.0
/// says can be `null`, as while the model loads. By default the id is the
/// path the model was loaded from.
fn models_fixture(meta: bool) -> Value {
    json!({
        "models": [{
            "name": "/models/Qwen3-8B-Q4_K_M.gguf", "model": "/models/Qwen3-8B-Q4_K_M.gguf",
            "modified_at": "", "size": "", "digest": "", "type": "model",
            "description": "", "tags": [""], "capabilities": ["completion"],
            "parameters": "",
            "details": {"parent_model": "", "format": "gguf", "family": "",
                        "families": [""], "parameter_size": "", "quantization_level": ""}
        }],
        "object": "list",
        "data": [{
            "id": "/models/Qwen3-8B-Q4_K_M.gguf",
            "aliases": [],
            "tags": [],
            "object": "model",
            "architecture": {"input_modalities": ["text"], "output_modalities": ["text"]},
            "created": 1763100000,
            "owned_by": "llamacpp",
            "meta": if meta {
                json!({"vocab_type": 2, "n_vocab": 151936, "n_ctx": 32768,
                       "n_ctx_train": 40960, "n_embd": 4096,
                       "n_params": 8190735360_u64, "size": 5027783488_u64,
                       "ftype": "Q4_K - Medium"})
            } else {
                Value::Null
            }
        }]
    })
}

/// A llama-server that answers by path and keeps every path asked, as
/// `Adapter::get` and `get_json` would fetch them. A path with no reply
/// fails, as a disabled endpoint does.
#[derive(Default)]
struct Server {
    replies: RefCell<HashMap<&'static str, String>>,
    asked: RefCell<Vec<String>>,
}

impl Server {
    fn with(replies: &[(&'static str, String)]) -> Self {
        let server = Self::default();
        for (path, body) in replies {
            server.set(path, Some(body.clone()));
        }
        server
    }

    fn set(&self, path: &'static str, body: Option<String>) {
        let mut replies = self.replies.borrow_mut();
        match body {
            Some(body) => replies.insert(path, body),
            None => replies.remove(path),
        };
    }

    fn text(&self, path: &str) -> Option<String> {
        self.asked.borrow_mut().push(path.into());
        self.replies.borrow().get(path).cloned()
    }

    fn json(&self, path: &str) -> Option<Value> {
        self.text(path)
            .and_then(|body| serde_json::from_str(&body).ok())
    }

    fn poll(&self, llama: &mut LlamaRate, now: Instant) -> Option<LlmTelemetry> {
        llama.poll(now, &|path| self.text(path), &|path| self.json(path))
    }

    fn count(&self, path: &str) -> usize {
        self.asked
            .borrow()
            .iter()
            .filter(|asked| *asked == path)
            .count()
    }

    fn forget(&self) {
        self.asked.borrow_mut().clear();
    }
}

/// A busy four-slot server answering every endpoint.
fn busy_server() -> Server {
    Server::with(&[
        (
            "/metrics",
            metrics_text(&Scrape {
                processing: 2,
                ..Scrape::default()
            }),
        ),
        ("/props", props_fixture(false).to_string()),
        ("/v1/models", models_fixture(true).to_string()),
        ("/slots", slots_fixture().to_string()),
    ])
}

#[test]
fn doubles_print_as_llama_server_prints_them() {
    // What a C++ `std::stringstream << double` printed for each, built with
    // Apple clang on 2026-11-15.
    for (value, printed) in [
        (18432.0, "18432"),
        (23.04, "23.04"),
        (1.6, "1.6"),
        (1234567.0, "1.23457e+06"),
        (12345678.0, "1.23457e+07"),
        (0.0, "0"),
        (102.4, "102.4"),
        (999999.0, "999999"),
        (1000000.0, "1e+06"),
        (0.5, "0.5"),
    ] {
        assert_eq!(g6(value), printed);
    }
    // The parser reads them back as Prometheus values.
    assert_eq!(
        metric(
            "llamacpp:n_decode_total 1.23457e+06\n",
            "llamacpp:n_decode_total"
        ),
        Some(1234570.0)
    );
}

#[test]
fn answering_slots_become_requests_with_their_exact_prompt_and_cache() {
    let server = busy_server();
    let mut llama = LlamaRate::default();
    let start = Instant::now();
    let first = server.poll(&mut llama, start).unwrap();
    // Slot 0 answers; slot 1 still reads its prompt, whose length `/slots`
    // does not give, so it has no row; idle slots have none either.
    assert_eq!(first.requests.len(), 1);
    let request = &first.requests[0];
    assert_eq!(request.provider, "llama.cpp");
    assert_eq!(request.model, "Qwen3-8B-Q4_K_M.gguf");
    assert_eq!(request.id, "4211");
    assert_eq!(
        request.prompt, 30720,
        "the cached prefix plus what it computed"
    );
    assert_eq!(request.cached, Some(24576));
    assert_eq!(request.output, Some(293));
    assert_eq!(request.output_tps, None, "one sample has no speed yet");
    assert!(!request.completed && !request.queued);
    assert_eq!(request.prefilled, None);
    assert_eq!(first.active_requests, Some(2));
    assert_eq!(first.output_tokens, Some(293));
    assert_eq!(
        first.prompt_tokens, None,
        "a slot's context is not its prompt"
    );
    assert_eq!(first.model.as_deref(), Some("Qwen3-8B-Q4_K_M.gguf"));
    // The prompt being read: what its slot holds, and its cached share.
    assert_eq!(first.llama.reading, Some(12288));
    assert_eq!(first.llama.reading_cached, Some(8192));
    assert_eq!(first.prefill_tokens_done, None);
    // The running request nearest its limit: slot 0, 32,768 less 31,012.
    assert_eq!(first.kv_free_tokens, Some(1756));
    // A second sample a second later: the request's own speed.
    let mut slots = slots_fixture();
    slots[0]["next_token"][0]["n_decoded"] = json!(320);
    slots[0]["n_prompt_tokens"] = json!(31039);
    server.set("/slots", Some(slots.to_string()));
    let second = server
        .poll(&mut llama, start + Duration::from_secs(1))
        .unwrap();
    assert_eq!(second.requests[0].output_tps, Some(27.0));
    assert_eq!(second.generation_tps, Some(27.0));
    assert!(second.generation_tps_live);
    assert_eq!(second.kv_free_tokens, Some(1729));
}

#[test]
fn slots_of_older_builds_give_no_rows_rather_than_guessed_prompts() {
    // Before `n_prompt_tokens*`, a running slot said only what it decoded.
    let server = busy_server();
    server.set(
        "/slots",
        Some(
            json!([{"id": 0, "id_task": 7, "is_processing": true,
                    "next_token": {"n_decoded": 40}}])
            .to_string(),
        ),
    );
    let mut llama = LlamaRate::default();
    let result = server.poll(&mut llama, Instant::now()).unwrap();
    assert!(result.requests.is_empty());
    assert_eq!(result.output_tokens, Some(40));
    assert_eq!(result.kv_free_tokens, None, "no room is known without use");
    assert_eq!(result.llama.reading, None);
}

#[test]
fn an_idle_server_is_not_asked_for_its_slots() {
    // With `/metrics` saying nothing is in flight there is nothing to read,
    // and a read would restart the server's idle timer.
    let server = busy_server();
    server.set("/metrics", Some(metrics_text(&Scrape::default())));
    let mut llama = LlamaRate::default();
    let start = Instant::now();
    for second in 0..30 {
        let result = server
            .poll(&mut llama, start + Duration::from_secs(second))
            .unwrap();
        assert_eq!(result.status.as_deref(), Some("idle"));
        assert_eq!(result.active_requests, Some(0));
        // A slot's whole context, which the next request may use.
        assert_eq!(result.kv_free_tokens, Some(32768));
        assert!(result.requests.is_empty());
        assert!(!result.llama.asleep);
    }
    assert_eq!(server.count("/slots"), 0);
    assert_eq!(server.count("/metrics"), 30);
    // What it serves is asked every ten seconds, never every poll.
    assert_eq!(server.count("/props"), 3);
    assert_eq!(server.count("/v1/models"), 3);
}

#[test]
fn a_sleeping_server_is_not_woken_and_reads_as_asleep() {
    // Without `/metrics` (it needs `--metrics`), `/slots` is the only window
    // on requests, and asking it for its slots would load the model again.
    let server = Server::with(&[
        ("/props", props_fixture(true).to_string()),
        ("/v1/models", models_fixture(true).to_string()),
        ("/slots", slots_fixture().to_string()),
    ]);
    let mut llama = LlamaRate::default();
    let start = Instant::now();
    for second in 0..60 {
        let result = server
            .poll(&mut llama, start + Duration::from_secs(second))
            .expect("asleep is not a failure");
        assert!(result.llama.asleep);
        assert_eq!(result.status.as_deref(), Some("idle"));
        assert_eq!(result.active_requests, Some(0));
        assert_eq!(result.kv_free_tokens, None, "nothing is allocated");
        // Its facts are still known: they come from replies that do not wake it.
        assert_eq!(result.model_facts.context_configured, Some(32768));
    }
    assert_eq!(server.count("/slots"), 0, "{:?}", server.asked.borrow());
    // Asked whether a request woke it every five seconds, not every poll.
    assert_eq!(server.count("/props"), 12);
    // A request wakes it: the next recheck sees it, and its slots are read.
    server.set("/props", Some(props_fixture(false).to_string()));
    server.forget();
    let awake = (60..66)
        .map(|second| {
            server
                .poll(&mut llama, start + Duration::from_secs(second))
                .unwrap()
        })
        .last()
        .unwrap();
    assert!(!awake.llama.asleep);
    assert!(server.count("/slots") >= 1);
    assert_eq!(awake.requests.len(), 1);
}

#[test]
fn with_metrics_a_sleeping_server_reads_as_asleep_until_a_request_wakes_it() {
    // A sleeping server answers `/metrics` from the scrape it kept, with
    // nothing in flight (`use_cached_metrics`).
    let server = Server::with(&[
        ("/metrics", metrics_text(&Scrape::default())),
        ("/props", props_fixture(true).to_string()),
        ("/v1/models", models_fixture(true).to_string()),
        ("/slots", slots_fixture().to_string()),
    ]);
    let mut llama = LlamaRate::default();
    let start = Instant::now();
    for second in 0..20 {
        let result = server
            .poll(&mut llama, start + Duration::from_secs(second))
            .unwrap();
        assert!(result.llama.asleep);
        assert_eq!(result.status.as_deref(), Some("idle"));
        assert_eq!(result.kv_free_tokens, None);
    }
    assert_eq!(server.count("/slots"), 0);
    assert_eq!(server.count("/props"), 2);
    // A request arrives: `/metrics` counts it at once, and only then are the
    // slots read.
    server.set(
        "/metrics",
        Some(metrics_text(&Scrape {
            processing: 2,
            ..Scrape::default()
        })),
    );
    let busy = server
        .poll(&mut llama, start + Duration::from_secs(20))
        .unwrap();
    assert!(!busy.llama.asleep);
    assert_eq!(server.count("/slots"), 1);
    assert_eq!(busy.requests.len(), 1);
}

#[test]
fn without_metrics_an_awake_server_is_read_every_poll_and_asked_after_a_gap() {
    let server = Server::with(&[
        ("/props", props_fixture(false).to_string()),
        ("/v1/models", models_fixture(true).to_string()),
        ("/slots", idle_slots().to_string()),
    ]);
    let mut llama = LlamaRate::default();
    let start = Instant::now();
    for second in 0..9 {
        let result = server
            .poll(&mut llama, start + Duration::from_secs(second))
            .unwrap();
        assert_eq!(result.status.as_deref(), Some("idle"));
        assert_eq!(result.kv_free_tokens, Some(32768));
    }
    // Each read restarted its idle timer: no need to ask whether it sleeps.
    assert_eq!(server.count("/slots"), 9);
    assert_eq!(server.count("/props"), 1);
    // After a pause longer than a read keeps it awake, ask first.
    server.forget();
    server.set("/props", Some(props_fixture(true).to_string()));
    let result = server
        .poll(&mut llama, start + Duration::from_secs(12))
        .unwrap();
    assert_eq!(*server.asked.borrow(), ["/metrics", "/props", "/v1/models"]);
    assert!(result.llama.asleep);
}

#[test]
fn a_props_reply_that_fails_forgets_the_sleep_but_keeps_the_facts() {
    let server = Server::with(&[
        ("/props", props_fixture(true).to_string()),
        ("/v1/models", models_fixture(true).to_string()),
        ("/slots", idle_slots().to_string()),
    ]);
    let mut llama = LlamaRate::default();
    let start = Instant::now();
    assert!(server.poll(&mut llama, start).unwrap().llama.asleep);
    assert_eq!(server.count("/slots"), 0);
    // `/props` stops answering: whether the server sleeps is unknown, and an
    // older build without `/props` has no sleep to keep, so its slots are read.
    server.set("/props", None);
    let result = server
        .poll(&mut llama, start + Duration::from_secs(5))
        .unwrap();
    assert!(!result.llama.asleep);
    assert_eq!(server.count("/slots"), 1);
    assert_eq!(result.llama.slots, Some(4), "what /props said before");
    assert_eq!(
        result.model_facts.runtime_version.as_deref(),
        Some("llama.cpp b8240-4f1e2a7")
    );
}

#[test]
fn slots_turned_off_are_never_asked_for() {
    // `get_slots` waits for the server to wake before it checks
    // `--no-slots`: asking would only wake it.
    let mut props = props_fixture(false);
    props["endpoint_slots"] = json!(false);
    let server = busy_server();
    server.set("/props", Some(props.to_string()));
    let mut llama = LlamaRate::default();
    let start = Instant::now();
    for second in 0..5 {
        let result = server
            .poll(&mut llama, start + Duration::from_secs(second))
            .unwrap();
        assert_eq!(result.active_requests, Some(2), "from /metrics");
        assert!(result.requests.is_empty());
    }
    assert_eq!(server.count("/slots"), 0);
}

#[test]
fn a_server_that_answers_nothing_has_failed_and_one_that_describes_itself_has_not() {
    let mut llama = LlamaRate::default();
    let silent = Server::default();
    assert!(silent.poll(&mut llama, Instant::now()).is_none());
    // Both monitoring endpoints off, but the server is there.
    let mut props = props_fixture(false);
    props["endpoint_slots"] = json!(false);
    let described = Server::with(&[
        ("/props", props.to_string()),
        ("/v1/models", models_fixture(true).to_string()),
    ]);
    let mut llama = LlamaRate::default();
    let result = described.poll(&mut llama, Instant::now()).unwrap();
    assert_eq!(result.status.as_deref(), Some("running"));
    assert_eq!(result.active_requests, None, "unknown, not zero");
    assert_eq!(result.model.as_deref(), Some("Qwen3-8B-Q4_K_M.gguf"));
    assert_eq!(result.model_facts.parameters, Some(8190735360));
}

#[test]
fn model_facts_come_from_v1_models_meta_and_props() {
    let server = busy_server();
    let mut llama = LlamaRate::default();
    let facts = server.poll(&mut llama, Instant::now()).unwrap().model_facts;
    assert_eq!(facts.parameters, Some(8_190_735_360));
    assert_eq!(facts.loaded_bytes, Some(5_027_783_488));
    let bits = facts.bits_per_weight.unwrap();
    assert!((bits - 4.9107).abs() < 1e-3, "{bits}");
    assert_eq!(facts.quantization.as_deref(), Some("Q4_K - Medium"));
    assert_eq!(facts.context_configured, Some(32768));
    assert_eq!(facts.context_trained, Some(40960));
    assert_eq!(
        facts.runtime_version.as_deref(),
        Some("llama.cpp b8240-4f1e2a7")
    );
    assert_eq!(facts.draft, Some(true), "a slot drafts speculatively");
    // All the weights, not what one pass reads: a mixture of experts reads
    // a few experts, and nothing here says whether this is one.
    assert_eq!(facts.bytes_per_pass, None);
    assert_eq!(facts.active_parameters, None);
    assert_eq!(
        facts.kv_bytes_per_token(),
        None,
        "no API gives the KV layout"
    );
    assert_eq!(
        crate::model_facts::lines(&facts),
        [
            "Model: 4.91 bits per weight, Q4_K - Medium · 8.2B parameters · draft",
            "Context: 32.8k tokens allowed of 41.0k trained",
            "Version: llama.cpp b8240-4f1e2a7",
        ]
    );
}

#[test]
fn a_loading_model_keeps_what_props_says_and_nothing_more() {
    let server = Server::with(&[
        ("/metrics", metrics_text(&Scrape::default())),
        ("/props", props_fixture(false).to_string()),
        ("/v1/models", models_fixture(false).to_string()),
    ]);
    let mut llama = LlamaRate::default();
    let result = server.poll(&mut llama, Instant::now()).unwrap();
    let facts = result.model_facts;
    assert_eq!(facts.parameters, None);
    assert_eq!(facts.bits_per_weight, None);
    assert_eq!(facts.loaded_bytes, None);
    assert_eq!(facts.context_trained, None);
    assert_eq!(facts.quantization.as_deref(), Some("Q4_K - Medium"));
    assert_eq!(facts.context_configured, Some(32768));
    assert_eq!(result.model.as_deref(), Some("Qwen3-8B-Q4_K_M.gguf"));
    // A reply that fails later keeps what the last one said.
    server.set("/v1/models", None);
    let later = server
        .poll(&mut llama, Instant::now() + Duration::from_secs(11))
        .unwrap();
    assert_eq!(later.model.as_deref(), Some("Qwen3-8B-Q4_K_M.gguf"));
}

#[test]
fn model_names_keep_a_path_s_file_name_and_unknown_types_stay_unknown() {
    assert_eq!(
        model_name("/Users/someone/models/Qwen3-8B-Q4_K_M.gguf").as_deref(),
        Some("Qwen3-8B-Q4_K_M.gguf")
    );
    assert_eq!(
        model_name(r"C:\models\gemma-3-4b.gguf").as_deref(),
        Some("gemma-3-4b.gguf")
    );
    assert_eq!(model_name("qwen3-coder").as_deref(), Some("qwen3-coder"));
    assert_eq!(model_name("/models/"), None);
    assert_eq!(model_name(""), None);
    assert_eq!(ftype(&json!("unknown, may not work")), None);
    assert_eq!(ftype(&json!("(guessed) unknown, may not work")), None);
    assert_eq!(
        ftype(&json!("(guessed) Q4_K - Medium")).as_deref(),
        Some("(guessed) Q4_K - Medium")
    );
    assert_eq!(ftype(&json!(4)), None);
    // A router lists several models: none of them is the one served.
    let router = json!({"data": [{"id": "a"}, {"id": "b"}]});
    assert_eq!(Catalogue::read(&router), Catalogue::default());
}

#[test]
fn draft_presence_is_known_from_slots_or_finished_requests() {
    let speculative = |value: bool| {
        let mut slots = slots_fixture();
        for slot in slots.as_array_mut().unwrap() {
            slot["speculative"] = json!(value);
        }
        slots.to_string()
    };
    let server = busy_server();
    let mut llama = LlamaRate::default();
    server.set("/slots", Some(speculative(false)));
    let none = server.poll(&mut llama, Instant::now()).unwrap();
    assert_eq!(none.model_facts.draft, Some(false));
    // Without the slots, the counters: a verify pass means a draft.
    assert_eq!(drafts_after(Some(1200.0), 4096.0), Some(true));
    // Answers written without one: nothing drafts.
    assert_eq!(drafts_after(Some(0.0), 4096.0), Some(false));
    // Nothing finished yet: unknown.
    assert_eq!(drafts_after(Some(0.0), 0.0), None);
    // A build without speculative counters says nothing.
    assert_eq!(drafts_after(None, 4096.0), None);
}

/// Whether a server whose `/slots` was never read drafts, by its counters:
/// `passes` verify passes and `predicted` tokens written.
fn drafts_after(passes: Option<f64>, predicted: f64) -> Option<bool> {
    let mut llama = LlamaRate::default();
    llama.averages(&format!(
        "llamacpp:tokens_predicted_total {predicted}\n\
         llamacpp:tokens_predicted_seconds_total 1\n\
         llamacpp:prompt_tokens_total 1\n\
         llamacpp:prompt_seconds_total 1\n"
    ));
    llama.drafts(&Counters {
        passes,
        ..Counters::default()
    })
}

#[test]
fn tokens_per_pass_is_the_growth_of_the_speculative_counters() {
    let mut llama = LlamaRate::default();
    // The first scrape only sets where the average starts.
    assert_eq!(llama.tokens_per_pass(Some(2880.0), Some(1200.0)), None);
    // Finished requests verified 240 passes and accepted 600 drafted tokens
    // in them: 1 + 600 / 240.
    assert_eq!(llama.tokens_per_pass(Some(3480.0), Some(1440.0)), Some(3.5));
    // Nothing finished since: unknown, and the average starts where it was.
    assert_eq!(llama.tokens_per_pass(Some(3480.0), Some(1440.0)), None);
    assert_eq!(llama.tokens_per_pass(Some(3500.0), Some(1450.0)), Some(3.0));
    // A pass that accepted nothing still emits its own token.
    assert_eq!(llama.tokens_per_pass(Some(3500.0), Some(1460.0)), Some(1.0));
    // A restart counts from zero again.
    assert_eq!(llama.tokens_per_pass(Some(10.0), Some(5.0)), None);
    assert_eq!(llama.tokens_per_pass(Some(30.0), Some(10.0)), Some(5.0));
    // A build without the counters, or a scrape that failed.
    assert_eq!(llama.tokens_per_pass(None, Some(10.0)), None);
    assert_eq!(llama.tokens_per_pass(Some(40.0), Some(15.0)), None);
}

#[test]
fn tokens_per_pass_waits_while_large_counters_move_in_printed_steps() {
    // From a million up llama-server prints six significant digits, so a
    // counter moves in steps of ten: a few passes cannot be read.
    let printed = |value: f64| metric(&format!("x {}\n", g6(value)), "x").unwrap();
    let mut llama = LlamaRate::default();
    let (accepted, passes) = (2_469_135.0, 1_000_003.0);
    assert_eq!(
        llama.tokens_per_pass(Some(printed(accepted)), Some(printed(passes))),
        None
    );
    // 300 more passes: within the steps' error, so it waits.
    assert_eq!(
        llama.tokens_per_pass(
            Some(printed(accepted + 450.0)),
            Some(printed(passes + 300.0))
        ),
        None
    );
    // 3,000 more: readable to within a hundredth.
    let average = llama
        .tokens_per_pass(
            Some(printed(accepted + 4500.0)),
            Some(printed(passes + 3000.0)),
        )
        .unwrap();
    assert!((average - 2.5).abs() < 0.01, "{average}");
    assert_eq!(printed_error(999_999.0), 0.0);
    assert_eq!(printed_error(1_000_000.0), 5.0);
    assert_eq!(printed_error(12_345_678.0), 50.0);
    assert!((printed_error(23.04) - 0.00005).abs() < 1e-12);
}

#[test]
fn tokens_per_pass_and_cache_totals_come_through_a_poll() {
    let server = busy_server();
    let mut llama = LlamaRate::default();
    let start = Instant::now();
    let first = server.poll(&mut llama, start).unwrap();
    assert_eq!(first.tokens_per_pass, None);
    // Prompt tokens served from the cache, over all served.
    assert_eq!(first.total_prompt_tokens, Some(18432 + 61440));
    assert_eq!(first.total_cached_tokens, Some(61440));
    let hit = first.prefix_hit_rate.unwrap();
    assert!((hit - 61440.0 / 79872.0 * 100.0).abs() < 1e-9, "{hit}");
    assert_eq!(first.cache_efficiency, first.prefix_hit_rate);
    // Since the server started: 1 + 2,880 / 1,200, and 2,880 of 4,800.
    assert!((first.llama.draft_per_pass.unwrap() - 3.4).abs() < 1e-9);
    assert!((first.llama.draft_acceptance.unwrap() - 0.6).abs() < 1e-9);
    assert_eq!(first.llama.busy_per_decode, Some(1.6));
    assert_eq!(first.llama.slots, Some(4));
    server.set(
        "/metrics",
        Some(metrics_text(&Scrape {
            processing: 2,
            accepted: 3480.0,
            passes: 1440.0,
            drafted: 5760.0,
            ..Scrape::default()
        })),
    );
    let second = server
        .poll(&mut llama, start + Duration::from_secs(1))
        .unwrap();
    assert_eq!(second.tokens_per_pass, Some(3.5));
}

#[test]
fn a_server_that_has_read_no_prompt_has_no_cache_hit_rate() {
    let server = busy_server();
    server.set(
        "/metrics",
        Some(metrics_text(&Scrape {
            prompt: 0.0,
            cached: 0.0,
            ..Scrape::default()
        })),
    );
    let mut llama = LlamaRate::default();
    let result = server.poll(&mut llama, Instant::now()).unwrap();
    assert_eq!(result.total_prompt_tokens, Some(0));
    assert_eq!(result.total_cached_tokens, Some(0));
    assert_eq!(result.prefix_hit_rate, None, "no share of nothing");
    assert_eq!(result.cache_efficiency, None);
}

#[test]
fn readings_without_metrics_or_with_older_metrics_stay_unknown() {
    // A build before the cache and speculative counters (2026-08).
    let server = busy_server();
    server.set(
        "/metrics",
        Some(
            "llamacpp:prompt_tokens_total 100\nllamacpp:requests_processing 1\n\
             llamacpp:requests_deferred 0\nllamacpp:n_busy_slots_per_decode 0\n"
                .into(),
        ),
    );
    let mut llama = LlamaRate::default();
    let result = server.poll(&mut llama, Instant::now()).unwrap();
    assert_eq!(result.total_prompt_tokens, None);
    assert_eq!(result.prefix_hit_rate, None);
    assert_eq!(result.tokens_per_pass, None);
    assert_eq!(result.llama.draft_per_pass, None);
    assert_eq!(result.llama.draft_acceptance, None);
    assert_eq!(
        result.llama.busy_per_decode, None,
        "no decode yet is no figure"
    );
}

#[test]
fn a_restarted_server_gives_its_requests_new_ids() {
    let server = busy_server();
    let mut llama = LlamaRate::default();
    let start = Instant::now();
    assert_eq!(
        server.poll(&mut llama, start).unwrap().requests[0].id,
        "4211"
    );
    // The same slot now runs task 7: task numbers started again.
    let mut slots = slots_fixture();
    slots[0]["id_task"] = json!(7);
    server.set("/slots", Some(slots.to_string()));
    let after = server
        .poll(&mut llama, start + Duration::from_secs(1))
        .unwrap();
    assert_eq!(after.requests[0].id, "7-1");
    // Counters that went back say so too.
    server.set(
        "/metrics",
        Some(metrics_text(&Scrape {
            processing: 2,
            predicted: 10.0,
            prompt: 10.0,
            ..Scrape::default()
        })),
    );
    let again = server
        .poll(&mut llama, start + Duration::from_secs(2))
        .unwrap();
    assert_eq!(again.requests[0].id, "7-2");
    // A later task in the same slot is the same server.
    slots[0]["id_task"] = json!(9);
    server.set("/slots", Some(slots.to_string()));
    let same = server
        .poll(&mut llama, start + Duration::from_secs(3))
        .unwrap();
    assert_eq!(same.requests[0].id, "9-2");
}

#[test]
fn diagnostics_say_what_llama_server_does_and_nothing_for_other_runtimes() {
    let mut sample = Sample {
        llm_provider: "llama.cpp".into(),
        llm_active_requests: Some(2),
        llm_kv_free_tokens: Some(1756),
        llama: LlamaReadings {
            slots: Some(4),
            busy_per_decode: Some(1.6),
            reading: Some(12288),
            reading_cached: Some(8192),
            draft_per_pass: Some(3.4),
            draft_acceptance: Some(0.6),
            ..LlamaReadings::default()
        },
        ..Sample::default()
    };
    assert_eq!(
        lines(&sample),
        [
            "Context left: 1.8k tokens, for the running request nearest its limit",
            "Reading a prompt: 12.3k tokens so far, 8.2k of them from the cache",
            "Slots: 4 · 1.6 busy per decode step since the server started",
            "Speculative decoding: 3.4 tokens per verify pass, 60% of drafted tokens accepted, since the server started",
        ]
    );
    // Idle: no request has context to lose, and nothing is being read.
    sample.llm_active_requests = Some(0);
    sample.llama.reading = None;
    sample.llama.busy_per_decode = None;
    sample.llama.draft_acceptance = None;
    assert_eq!(
        lines(&sample),
        [
            "Slots: 4",
            "Speculative decoding: 3.4 tokens per verify pass since the server started",
        ]
    );
    // One slot never shares a step; a prompt read without the cache.
    sample.llama = LlamaReadings {
        slots: Some(1),
        busy_per_decode: Some(1.0),
        reading: Some(500),
        reading_cached: Some(0),
        asleep: true,
        ..LlamaReadings::default()
    };
    assert_eq!(
        lines(&sample),
        [
            "Asleep: the model is unloaded until the next request loads it again; mlxtop does not wake it.",
            "Reading a prompt: 500 tokens so far",
        ]
    );
    sample.llm_provider = "oMLX".into();
    assert!(lines(&sample).is_empty());
}

#[test]
fn readings_survive_a_recording() {
    let readings = LlamaReadings {
        asleep: true,
        slots: Some(4),
        draft_per_pass: Some(3.4),
        ..LlamaReadings::default()
    };
    let text = serde_json::to_string(&readings).unwrap();
    assert_eq!(text, r#"{"asleep":true,"slots":4,"draft_per_pass":3.4}"#);
    assert_eq!(
        serde_json::from_str::<LlamaReadings>(&text).unwrap(),
        readings
    );
    assert_eq!(
        serde_json::to_string(&LlamaReadings::default()).unwrap(),
        "{}"
    );
}
