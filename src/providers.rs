// SPDX-License-Identifier: MIT
//! Read-only provider adapters. Only counters and identifiers cross into samples.
use crate::domain::{LlmTelemetry, RequestUsage, TelemetrySource};
use crate::json::{json_value, request_rate};
use crate::runtime_diagnostics::{default_port, ProbeIssue, RuntimeReport};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use std::{env, fs};

#[path = "provider_native.rs"]
mod native;

const MAX_USAGE_BYTES: u64 = 256 * 1024;
const MLX_SERVE_PORT: u16 = 11234;

pub(super) fn new_request_summary(
    seen: &mut VecDeque<(String, u64)>,
    request: &RequestUsage,
) -> Option<String> {
    let key = format!("{}\0{}\0{}", request.provider, request.model, request.id);
    if seen
        .iter()
        .any(|entry| entry == &(key.clone(), request.prompt))
    {
        return None;
    }
    seen.push_back((key, request.prompt));
    while seen.len() > 512 {
        seen.pop_front();
    }
    Some(request.summary())
}

fn counter(value: &Value, path: &[&str]) -> Option<u64> {
    json_value(value, path)?.as_u64()
}

fn identifier(value: &Value, field: &str) -> Option<String> {
    let value = value.get(field)?;
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    let text: String = text.chars().filter(|c| !c.is_control()).take(120).collect();
    (!text.is_empty()).then_some(text)
}

/// oMLX's admin rows for a distributed model carry this placeholder instead of
/// the request ID, so successive cluster requests would share one identity.
const OMLX_SYNTHETIC_CLUSTER_ID: &str = "rank0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OmlxPhase {
    Generating,
    Prefilling,
    Waiting,
}

/// One live oMLX request, whether reported by the local scheduler or by rank
/// zero of a distributed deployment.
#[derive(Clone, Debug)]
pub(super) struct OmlxRequest {
    pub phase: OmlxPhase,
    /// `None` when the server exposes no identity for the request.
    pub id: Option<String>,
    pub prompt: Option<u64>,
    pub cached: Option<u64>,
    pub output: Option<u64>,
    pub rate: Option<f64>,
    /// Age of rank zero's telemetry marker; scheduler rows are current.
    pub age: Option<Duration>,
}

fn omlx_prefill_rate(progress: &Value) -> Option<f64> {
    // oMLX's progress tracker initializes `speed` to zero until two chunk
    // updates establish a rate. That sentinel is not a measured zero tok/s.
    request_rate(progress, &["speed"])
        .filter(|speed| *speed > 0.0)
        .or_else(|| {
            request_rate(
                progress,
                &[
                    "tokens_per_second",
                    "prefill_tps",
                    "prompt_tokens_per_second",
                ],
            )
        })
}

/// Rank zero's latest metrics, unless its heartbeat is stale.
fn omlx_cluster_live(model: &Value) -> Option<(&Value, Option<Duration>)> {
    let live = model.pointer("/cluster/live")?;
    if live.get("stale").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let metrics = live.get("metrics").filter(|metrics| metrics.is_object())?;
    let age = live
        .get("age_seconds")
        .and_then(Value::as_f64)
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok());
    Some((metrics, age))
}

/// Normalize a model's generating, prefilling and waiting rows. Distributed
/// rows are replaced by rank zero's per-request metrics, which carry the real
/// request IDs; counting them once avoids duplicating work seen on every rank.
pub(super) fn omlx_model_requests(model: &Value) -> Vec<OmlxRequest> {
    let distributed = model
        .get("cluster")
        .is_some_and(|cluster| !cluster.is_null());
    let cluster = omlx_cluster_live(model);
    let cluster_requests = cluster.and_then(|(metrics, _)| {
        metrics
            .get("active_request_metrics")
            .and_then(Value::as_array)
    });
    let mut result = Vec::new();
    for (phase, field, rate_keys) in [
        (
            OmlxPhase::Generating,
            "generating",
            &["tokens_per_second"][..],
        ),
        (OmlxPhase::Prefilling, "prefilling", &[][..]),
        (OmlxPhase::Waiting, "waiting", &[][..]),
    ] {
        let Some(requests) = model.get(field).and_then(Value::as_array) else {
            continue;
        };
        for request in requests {
            let mut id = identifier(request, "request_id");
            let mut age = None;
            if distributed && id.as_deref() == Some(OMLX_SYNTHETIC_CLUSTER_ID) {
                if cluster_requests.is_some() {
                    continue;
                }
                // Without per-request metrics, the placeholder row was built
                // from rank zero's most recent running request.
                id = cluster
                    .and_then(|(metrics, _)| metrics.get("last_request"))
                    .filter(|last| last.get("status").and_then(Value::as_str) == Some("running"))
                    .and_then(|last| identifier(last, "request_id"));
                age = cluster.and_then(|(_, age)| age);
            }
            result.push(OmlxRequest {
                phase,
                id,
                prompt: counter(request, &["prompt_tokens"]),
                cached: counter(request, &["cached_tokens"]),
                output: counter(request, &["generated_tokens"]),
                rate: if phase == OmlxPhase::Prefilling {
                    omlx_prefill_rate(request)
                } else {
                    request_rate(request, rate_keys)
                },
                age,
            });
        }
    }
    let Some((_, age)) = cluster else {
        return result;
    };
    for request in cluster_requests.into_iter().flatten() {
        if request.get("status").and_then(Value::as_str) != Some("running") {
            continue;
        }
        let progress = request.get("prefill_progress");
        let prefilling = progress
            .and_then(|progress| progress.get("active"))
            .and_then(Value::as_bool)
            == Some(true);
        result.push(OmlxRequest {
            phase: if prefilling {
                OmlxPhase::Prefilling
            } else {
                OmlxPhase::Generating
            },
            id: identifier(request, "request_id"),
            prompt: counter(request, &["prompt_tokens"]),
            cached: counter(request, &["cached_tokens"]),
            output: counter(request, &["completion_tokens"]),
            rate: if prefilling {
                progress.and_then(omlx_prefill_rate)
            } else {
                request_rate(request, &["decode_tps"])
            },
            age,
        });
    }
    result
}

pub(super) fn omlx_requests(stats: &Value) -> Vec<RequestUsage> {
    let Some(models) = stats
        .pointer("/active_models/models")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let now = SystemTime::now();
    let mut result = Vec::new();
    for model in models {
        for request in omlx_model_requests(model) {
            // Without an identity, one request cannot be told from the next.
            let (Some(id), Some(prompt)) = (request.id, request.prompt) else {
                continue;
            };
            // Waiting requests may not have been tokenized yet.
            if prompt == 0 && request.phase == OmlxPhase::Waiting {
                continue;
            }
            result.push(RequestUsage {
                provider: "oMLX".into(),
                model: identifier(model, "id").unwrap_or_else(|| "unknown".into()),
                id,
                prompt,
                cached: request.cached.filter(|n| *n <= prompt),
                output: request.output,
                output_tps: (request.phase == OmlxPhase::Generating)
                    .then_some(request.rate)
                    .flatten(),
                completed: false,
                ttft_ms: None,
                observed_at: Some(
                    request
                        .age
                        .and_then(|age| now.checked_sub(age))
                        .unwrap_or(now),
                ),
            });
        }
    }
    result
}

fn canonical_provider(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "omlx" => Some("oMLX"),
        "mlx-lm" | "mlx_lm.server" | "mlx_lm" => Some("mlx-lm"),
        "ollama" => Some("Ollama"),
        "llama.cpp" | "llama-server" => Some("llama.cpp"),
        "lm studio" | "lmstudio" | "llmster" => Some("LM Studio"),
        "koboldcpp" => Some("KoboldCpp"),
        "localai" => Some("LocalAI"),
        "vllm" => Some("vLLM"),
        "mlx-serve" | "mlx_serve" | "mlxserve" => Some("mlx-serve"),
        "sglang" => Some("SGLang"),
        "jan" => Some("Jan"),
        "gpt4all" => Some("GPT4All"),
        _ => None,
    }
}

pub(super) struct Adapter {
    configured: Option<String>,
    usage_file: Option<PathBuf>,
    selected: Option<String>,
    port: Option<u16>,
    cached: Option<LlmTelemetry>,
    next_poll: Instant,
    backoff: Duration,
    kobold_uptime: Option<f64>,
    kobold_session: u64,
    endpoint: native::Endpoint,
    metrics: native::MetricsHistory,
    diagnostics: RefCell<RuntimeReport>,
    config_error: Option<ProbeIssue>,
}

impl Adapter {
    pub fn new() -> Self {
        Self {
            configured: env::var("MLXTOP_PROVIDER").ok(),
            usage_file: env::var_os("MLXTOP_USAGE_FILE").map(PathBuf::from),
            selected: None,
            port: env::var("MLXTOP_PROVIDER_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|p| *p > 0),
            cached: None,
            next_poll: Instant::now(),
            backoff: Duration::from_secs(1),
            kobold_uptime: None,
            kobold_session: 0,
            endpoint: native::Endpoint::from_env(),
            metrics: native::MetricsHistory::default(),
            diagnostics: RefCell::default(),
            config_error: env::var("MLXTOP_PROVIDER_PORT").ok().and_then(|s| {
                if s.parse::<u16>().is_ok_and(|p| p > 0) {
                    None
                } else {
                    Some(ProbeIssue::InvalidPort)
                }
            }),
        }
    }

    pub fn selected(&mut self, detected: Option<&str>) -> bool {
        let selected = self
            .configured
            .as_deref()
            .or(detected)
            .map(|s| canonical_provider(s).unwrap_or("unsupported").to_owned());
        if selected != self.selected {
            self.selected = selected;
            *self.diagnostics.borrow_mut() = RuntimeReport::default();
            self.cached = None;
            self.next_poll = Instant::now();
            self.backoff = Duration::from_secs(1);
            self.kobold_uptime = None;
            self.metrics = native::MetricsHistory::default();
            self.kobold_session = self.kobold_session.saturating_add(1);
        }
        self.usage_file.is_some() || self.selected.as_deref().is_some_and(|s| s != "oMLX")
    }

    pub(crate) fn report(&self) -> RuntimeReport {
        self.diagnostics.borrow().clone()
    }

    pub(crate) fn explicitly_selected(&self) -> bool {
        self.configured.is_some()
    }

    pub fn provider(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn poll(&mut self) -> Option<LlmTelemetry> {
        // Preserve recorder-only collection without claiming that an API was
        // contacted, or treating the intentionally absent API as a failure.
        if self.usage_file.is_some()
            && self.config_error.is_none()
            && matches!(self.selected.as_deref(), None | Some("oMLX"))
        {
            *self.diagnostics.borrow_mut() = RuntimeReport {
                provider: self.selected.clone(),
                selection: "MLXTOP_USAGE_FILE (recorder only)",
                recorder_only: true,
                ..RuntimeReport::default()
            };
            return self.with_usage();
        }
        let now = Instant::now();
        if now < self.next_poll {
            return self.with_usage();
        }
        {
            let mut report = self.diagnostics.borrow_mut();
            report.provider = self.selected.clone();
            report.selection = if self.configured.is_some() {
                "MLXTOP_PROVIDER"
            } else {
                "process detection"
            };
            report.endpoint = self
                .endpoint
                .base_url(self.port.unwrap_or(default_port(self.selected.as_deref())))
                .unwrap_or_else(|_| "<invalid endpoint>".into());
            report.remote = self.endpoint.is_remote();
            report.credentials_present = self.endpoint.api_key.is_some();
            report.begin();
        }
        let config_error = self.config_error.clone().or_else(|| {
            (self.selected.as_deref() == Some("unsupported")).then_some(ProbeIssue::InvalidProvider)
        });
        let result = if let Some(issue) = config_error {
            self.diagnostics
                .borrow_mut()
                .record::<()>("configuration", Err(issue));
            None
        } else {
            match self.selected.as_deref() {
                Some("KoboldCpp") => self.poll_kobold(),
                Some("llama.cpp") => self.poll_llama(),
                Some("vLLM" | "SGLang") => {
                    let provider = self.selected.as_deref().unwrap();
                    let port = if provider == "vLLM" { 8000 } else { 30000 };
                    let body = self.get(port, "/metrics");
                    body.and_then(|body| self.metrics.observe(provider, &body, now))
                }
                Some("mlx-serve") => self.poll_mlx_serve(now),
                Some("Ollama") => self
                    .get_json(11434, "/api/ps")
                    .and_then(|value| native::ollama(&value)),
                Some("LM Studio") => self
                    .get_json(1234, "/api/v1/models")
                    .and_then(|value| native::lm_studio(&value))
                    .or_else(|| {
                        self.get_json(1234, "/api/v0/models")
                            .and_then(|value| native::lm_studio_legacy(&value))
                    })
                    .or_else(|| self.poll_models("LM Studio", 1234)),
                Some("mlx-lm") => self.poll_models("mlx-lm", 8080),
                Some("LocalAI") => self.poll_models("LocalAI", 8080),
                Some("Jan") => self.poll_models("Jan", 6767),
                Some("GPT4All") => self.poll_models("GPT4All", 4891),
                _ => None,
            }
        };
        self.diagnostics.borrow_mut().finish(result.is_some());
        if let Some(mut result) = result {
            // A successful poll does not make KoboldCpp's last completion new.
            if result.provider.as_deref() == Some("KoboldCpp") {
                if let Some(previous) = &self.cached {
                    if result.requests.first().map(|r| &r.id)
                        == previous.requests.first().map(|r| &r.id)
                    {
                        result.observed_at = previous.observed_at;
                    }
                }
            }
            result.remote = self.endpoint.is_remote();
            if result.remote {
                let details = result.details.get_or_insert_with(String::new);
                *details = format!("remote API · {details}");
            }
            self.cached = Some(result);
            self.backoff = Duration::from_secs(1);
        } else {
            self.backoff = (self.backoff * 2).min(Duration::from_secs(30));
        }
        self.next_poll = now + self.backoff;
        self.with_usage()
    }

    fn with_usage(&self) -> Option<LlmTelemetry> {
        let reported = self
            .usage_file
            .as_ref()
            .and_then(|path| read_usage_file(path, self.selected.as_deref()));
        {
            let mut report = self.diagnostics.borrow_mut();
            report.usage_file(self.usage_file.as_deref(), reported.is_some());
            if report.recorder_only && report.provider.is_none() {
                report.provider = reported.as_ref().and_then(|usage| usage.provider.clone());
            }
        }
        match (self.cached.clone(), reported) {
            (Some(mut native), Some(reported)) if native.source == TelemetrySource::Live => {
                // Completed prompt counts belong in request history. Never attach
                // them to the output/queue of an unrelated active request.
                native.requests.extend(reported.requests);
                Some(native)
            }
            (native, reported) => reported.or(native),
        }
    }

    fn get(&self, port: u16, path: &str) -> Option<String> {
        self.diagnostics
            .borrow_mut()
            .record(path, self.endpoint.get(self.port.unwrap_or(port), path))
    }

    fn get_json(&self, port: u16, path: &str) -> Option<Value> {
        let body = self.get(port, path)?;
        self.diagnostics.borrow_mut().record(
            path,
            serde_json::from_str(&body).map_err(|_| ProbeIssue::InvalidResponse),
        )
    }

    /// mlx-serve: Prometheus counters for rates and queues, and `/metrics.json`
    /// for the model and the live requests. Both need `--metrics`; the second is
    /// optional so an older server still reports rates.
    fn poll_mlx_serve(&mut self, now: Instant) -> Option<LlmTelemetry> {
        let body = self.get(MLX_SERVE_PORT, "/metrics")?;
        let mut result = self.metrics.observe("mlx-serve", &body, now)?;
        if self.metrics.sessions_known_missing() {
            return Some(result);
        }
        let reply = self
            .endpoint
            .get(self.port.unwrap_or(MLX_SERVE_PORT), "/metrics.json");
        if matches!(reply, Err(ProbeIssue::Http(404))) {
            self.metrics.note_sessions_missing();
        }
        let mut diagnostics = self.diagnostics.borrow_mut();
        let value = diagnostics.record("/metrics.json", reply).and_then(|body| {
            diagnostics.record(
                "/metrics.json",
                serde_json::from_str(&body).map_err(|_| ProbeIssue::InvalidResponse),
            )
        });
        if let Some(value) = value {
            native::apply_mlx_serve_sessions(&mut result, &value, SystemTime::now());
        }
        Some(result)
    }

    fn poll_models(&self, provider: &str, port: u16) -> Option<LlmTelemetry> {
        native::models(provider, &self.get_json(port, "/v1/models")?)
    }

    fn poll_kobold(&mut self) -> Option<LlmTelemetry> {
        let perf: Value = serde_json::from_str(&self.get(5001, "/api/extra/perf")?).ok()?;
        self.kobold_result(&perf)
    }

    fn kobold_result(&mut self, perf: &Value) -> Option<LlmTelemetry> {
        let mut result = parse_kobold(perf)?;
        let uptime = perf
            .get("uptime")
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite() && *n >= 0.0);
        if uptime
            .zip(self.kobold_uptime)
            .is_some_and(|(now, old)| now < old)
        {
            self.kobold_session = self.kobold_session.saturating_add(1);
        }
        self.kobold_uptime = uptime.or(self.kobold_uptime);
        for request in &mut result.requests {
            request.id = format!("session-{}-{}", self.kobold_session, request.id);
        }
        Some(result)
    }

    fn poll_llama(&self) -> Option<LlmTelemetry> {
        // Either monitoring endpoint may be disabled independently.
        let slots = self
            .get(8080, "/slots")
            .and_then(|body| serde_json::from_str::<Value>(&body).ok())
            .and_then(|slots| parse_llama_slots(&slots));
        let metrics = self.get(8080, "/metrics");
        merge_llama_metrics(slots, metrics.as_deref())
    }
}

fn merge_llama_metrics(slots: Option<LlmTelemetry>, metrics: Option<&str>) -> Option<LlmTelemetry> {
    let Some(metrics) = metrics else {
        return slots;
    };
    let generation = metric(metrics, "llamacpp:predicted_tokens_seconds");
    let prefill = metric(metrics, "llamacpp:prompt_tokens_seconds");
    let active = metric_count(metrics, "llamacpp:requests_processing");
    let waiting = metric_count(metrics, "llamacpp:requests_deferred");
    if generation.is_none() && prefill.is_none() && active.is_none() && waiting.is_none() {
        return slots;
    }
    let mut result = slots.unwrap_or_else(|| LlmTelemetry {
        source: TelemetrySource::Live,
        observed_at: Some(SystemTime::now()),
        provider: Some("llama.cpp".into()),
        ..LlmTelemetry::default()
    });
    result.generation_tps = generation;
    result.prefill_tps = prefill;
    result.active_requests = result.active_requests.or(active);
    result.waiting_requests = waiting;
    result.status = Some(
        match result.active_requests {
            Some(0) if waiting.unwrap_or(0) == 0 => "idle",
            Some(_) => "processing",
            None => "running",
        }
        .into(),
    );
    Some(result)
}

fn metric_count(text: &str, name: &str) -> Option<u64> {
    metric(text, name)
        .filter(|n| n.fract() == 0.0 && *n < u64::MAX as f64)
        .map(|n| n as u64)
}

fn metric(text: &str, name: &str) -> Option<f64> {
    native::metric_sum(text, name)
}

fn parse_llama_slots(slots: &Value) -> Option<LlmTelemetry> {
    let slots = slots.as_array()?;
    if slots
        .iter()
        .any(|slot| slot.get("is_processing").and_then(Value::as_bool).is_none())
    {
        return None;
    }
    let active: Vec<_> = slots
        .iter()
        .filter(|s| s["is_processing"] == true)
        .collect();
    Some(LlmTelemetry {
        source: TelemetrySource::Live,
        observed_at: Some(SystemTime::now()),
        provider: Some("llama.cpp".into()),
        status: Some(
            if active.is_empty() {
                "idle"
            } else {
                "processing"
            }
            .into(),
        ),
        active_requests: Some(active.len() as u64),
        // Sum every active slot, but never present a partial sum as complete.
        output_tokens: (!active.is_empty())
            .then(|| {
                active.iter().try_fold(0_u64, |sum, slot| {
                    sum.checked_add(counter(slot, &["next_token", "n_decoded"])?)
                })
            })
            .flatten(),
        // n_ctx is capacity, n_decoded is output, and prompt_n is fresh prefill
        // work. None is a substitute for a full per-request prompt count.
        ..LlmTelemetry::default()
    })
}

fn parse_kobold(perf: &Value) -> Option<LlmTelemetry> {
    let generations = counter(perf, &["total_gens"])?;
    let prompt = (generations > 0)
        .then(|| counter(perf, &["last_input_count"]))
        .flatten();
    let output = (generations > 0)
        .then(|| counter(perf, &["last_token_count"]))
        .flatten();
    // The endpoint describes the last result, even when another request runs.
    let requests = prompt
        .map(|prompt| RequestUsage {
            provider: "KoboldCpp".into(),
            model: "unknown".into(),
            id: format!("generation-{generations}"),
            prompt,
            cached: None,
            output,
            output_tps: request_rate(perf, &["last_eval_speed"]),
            completed: true,
            ttft_ms: None,
            observed_at: None,
        })
        .into_iter()
        .collect();
    Some(LlmTelemetry {
        source: TelemetrySource::Report,
        observed_at: Some(SystemTime::now()),
        provider: Some("KoboldCpp".into()),
        status: Some(
            if generations > 0 {
                "last result"
            } else {
                "idle"
            }
            .into(),
        ),
        prompt_tokens: prompt,
        output_tokens: output,
        requests,
        generation_tps: (generations > 0)
            .then(|| request_rate(perf, &["last_eval_speed"]))
            .flatten(),
        prefill_tps: (generations > 0)
            .then(|| request_rate(perf, &["last_process_speed"]))
            .flatten(),
        ..LlmTelemetry::default()
    })
}

fn parse_usage(record: &Value) -> Option<LlmTelemetry> {
    if record.get("done").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let provider = canonical_provider(record.get("provider")?.as_str()?)?;
    let id = identifier(record, "request_id")?;
    let timestamp = counter(record, &["observed_at"])?;
    let observed_at = SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(timestamp))?;
    if observed_at > SystemTime::now() {
        return None;
    }
    let usage = record
        .get("usage")
        .or_else(|| record.get("stats"))
        .unwrap_or(record);
    let prompt = counter(usage, &["prompt_tokens"])
        .or_else(|| counter(usage, &["input_tokens"]))
        .or_else(|| counter(usage, &["prompt_eval_count"]))?;
    let output = counter(usage, &["completion_tokens"])
        .or_else(|| counter(usage, &["output_tokens"]))
        .or_else(|| counter(usage, &["total_output_tokens"]))
        .or_else(|| counter(usage, &["eval_count"]));
    let cached = counter(usage, &["prompt_tokens_details", "cached_tokens"])
        .or_else(|| counter(usage, &["input_tokens_details", "cached_tokens"]))
        .or_else(|| counter(usage, &["cached_tokens"]))
        .or_else(|| counter(usage, &["prompt_eval_cached_count"]))
        .filter(|n| *n <= prompt);
    let model = identifier(record, "model")
        .or_else(|| identifier(record, "model_instance_id"))
        .unwrap_or_else(|| "unknown".into());
    let output_tps = record
        .get("timings")
        .and_then(|timings| request_rate(timings, &["output_tokens_per_second"]))
        .or_else(|| {
            (provider == "LM Studio")
                .then(|| request_rate(usage, &["tokens_per_second"]))
                .flatten()
        })
        .or_else(|| {
            // Ollama reports decode time in nanoseconds. Queue time, total
            // duration and prefill duration cannot substitute for it.
            if provider != "Ollama" {
                return None;
            }
            let duration = counter(usage, &["eval_duration"]).filter(|duration| *duration > 0)?;
            let count = counter(usage, &["eval_count"])?;
            Some(count as f64 / duration as f64 * 1_000_000_000.0)
        });
    Some(LlmTelemetry {
        source: TelemetrySource::Report,
        observed_at: Some(observed_at),
        provider: Some(provider.into()),
        model: Some(model.clone()),
        status: Some("last result".into()),
        prompt_tokens: Some(prompt),
        output_tokens: output,
        requests: vec![RequestUsage {
            provider: provider.into(),
            model,
            id,
            prompt,
            output,
            output_tps,
            cached,
            completed: true,
            ttft_ms: counter(record, &["timings", "time_to_first_token_ms"]).or_else(|| {
                (provider == "LM Studio")
                    .then(|| {
                        request_rate(usage, &["time_to_first_token_seconds"])
                            .and_then(native::seconds_to_ms)
                    })
                    .flatten()
            }),
            observed_at: Some(observed_at),
        }],
        ..LlmTelemetry::default()
    })
}

fn read_usage_file(path: &Path, provider: Option<&str>) -> Option<LlmTelemetry> {
    // Avoid blocking on a FIFO accidentally configured as a usage file.
    if !fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    let start = metadata.len().saturating_sub(MAX_USAGE_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_USAGE_BYTES).read_to_end(&mut bytes).ok()?;
    // Only newline-terminated records are complete. Skip any partial first line.
    let end = bytes.iter().rposition(|b| *b == b'\n')?;
    let start = if start > 0 {
        bytes.iter().position(|b| *b == b'\n')? + 1
    } else {
        0
    };
    if start > end {
        return None;
    }
    let mut latest: Option<LlmTelemetry> = None;
    let mut requests = Vec::new();
    for line in bytes[start..end].split(|b| *b == b'\n') {
        let Some(record) = serde_json::from_slice::<Value>(line)
            .ok()
            .and_then(|v| parse_usage(&v))
        else {
            continue;
        };
        if provider.is_some_and(|selected| record.provider.as_deref() != Some(selected)) {
            continue;
        }
        requests.extend(record.requests.clone());
        if latest
            .as_ref()
            .is_none_or(|old| record.observed_at >= old.observed_at)
        {
            latest = Some(record);
        }
    }
    let mut latest = latest?;
    let selected = latest.provider.as_deref();
    let mut seen = std::collections::HashSet::new();
    latest.requests = requests
        .into_iter()
        .rev()
        .filter(|request| Some(request.provider.as_str()) == selected)
        .filter(|request| {
            seen.insert((
                request.provider.clone(),
                request.model.clone(),
                request.id.clone(),
            ))
        })
        .take(128)
        .collect();
    latest.requests.reverse();
    Some(latest)
}

#[cfg(test)]
#[path = "tests/providers.rs"]
mod tests;
