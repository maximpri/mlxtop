// SPDX-License-Identifier: MIT
//! Native model inventories and Prometheus telemetry. No inference requests:
//! every request here is a GET but Ollama's read-only `POST /api/show`, sent
//! once per model.
use crate::domain::{LlmTelemetry, RequestUsage, Sample, TelemetrySource};
use crate::formatting::{bytes, compact_tokens};
use crate::json::json_value;
use crate::logging::{diagnostics_log, log_field};
use crate::model_facts::ModelFacts;
use crate::omlx::{is_local_host, is_loopback_host};
use crate::providers::{counter, identifier};
use crate::runtime_diagnostics::ProbeIssue;
use crate::transport::MAX_HTTP_RESPONSE_BYTES;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::env;
use std::time::{Duration, Instant, SystemTime};

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Routes mlxtop asks no runtime for, whatever answers on the port: GETs
/// that change state (SGLang's cache flush and its profiler, garbage
/// collector, slow-down, memory and session routes; llama-swap's unload and
/// its pass-through, which loads the model it names; KoboldCpp's shutdown;
/// Foundry Local's load, unload and device choice; exo's benchmark) and
/// SGLang's health check that generates a token. A route ending in `/` covers
/// every path under it. SGLang's plain `/health` generates a token too, but
/// llama.cpp and oMLX answer `/health` without one, so it is not listed: the
/// SGLang adapter asks for `/metrics` alone.
pub(super) const NEVER_ASKED: [&str; 18] = [
    "/health_generate",
    "/flush_cache",
    "/start_profile",
    "/stop_profile",
    "/freeze_gc",
    "/slow_down",
    "/release_memory_occupation",
    "/resume_memory_occupation",
    "/open_session",
    "/close_session",
    "/unload",
    "/upstream/",
    "/api/extra/shutdown",
    "/openai/load/",
    "/openai/unload/",
    "/openai/unloadall",
    "/openai/setgpudevice/",
    "/bench/",
];

/// Whether `path`, its query aside, is one of [`NEVER_ASKED`].
pub(super) fn never_asked(path: &str) -> bool {
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    NEVER_ASKED.iter().any(|route| {
        if route.ends_with('/') {
            path.starts_with(route)
        } else {
            path.strip_prefix(route)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        }
    })
}

#[derive(Default)]
pub(super) struct Endpoint {
    pub url: Option<String>,
    pub api_key: Option<String>,
    pub allow_remote_auth: bool,
}

impl Endpoint {
    /// The endpoint from the environment; the key from
    /// `MLXTOP_PROVIDER_API_KEY`, else the one set in mlxtop's Settings.
    pub fn from_env(configured_key: Option<String>) -> Self {
        Self {
            url: env::var("MLXTOP_PROVIDER_URL").ok(),
            api_key: env::var("MLXTOP_PROVIDER_API_KEY")
                .ok()
                .filter(|key| !key.trim().is_empty())
                .or(configured_key),
            allow_remote_auth: env::var("MLXTOP_ALLOW_REMOTE_AUTH").is_ok_and(|value| value == "1"),
        }
    }

    /// Whether the server runs on another machine, so its load is not this
    /// machine's hardware.
    pub fn is_remote(&self) -> bool {
        !self.host_is(is_local_host)
    }

    /// Credentials go only to loopback unless remote authentication is
    /// allowed, even to a server on another of this machine's addresses.
    fn off_loopback(&self) -> bool {
        !self.host_is(is_loopback_host)
    }

    /// `test` of the configured URL's host; true with no URL (the default
    /// is loopback) or one that does not parse.
    fn host_is(&self, test: fn(&str) -> bool) -> bool {
        self.url
            .as_deref()
            .and_then(|url| url.parse::<ureq::http::Uri>().ok())
            .and_then(|uri| uri.host().map(test))
            .unwrap_or(true)
    }

    pub fn base_url(&self, port: u16) -> Result<String, ProbeIssue> {
        let default = format!("http://127.0.0.1:{port}");
        let base = self
            .url
            .as_deref()
            .unwrap_or(&default)
            .trim_end_matches('/');
        crate::runtime_diagnostics::safe_endpoint(base).ok_or(ProbeIssue::InvalidEndpoint)
    }

    pub fn get(&self, port: u16, path: &str) -> Result<String, ProbeIssue> {
        let body = self.request(port, path, None)?;
        if path == OLLAMA_RUNNING {
            // The poll asks only for the models Ollama runs; what each one is
            // comes from `/api/show`, asked here on the same endpoint.
            ShownModels::learn(self, port, &body);
        }
        Ok(body)
    }

    /// A GET, or with `json` a POST of that body; refused for a route in
    /// [`NEVER_ASKED`].
    fn request(&self, port: u16, path: &str, json: Option<&str>) -> Result<String, ProbeIssue> {
        if never_asked(path) {
            return Err(ProbeIssue::InvalidEndpoint);
        }
        let base = self.base_url(port)?;
        if self.api_key.is_some() && self.off_loopback() && !self.allow_remote_auth {
            return Err(ProbeIssue::RemoteAuthBlocked);
        }
        let authorization = match &self.api_key {
            Some(key) if key.is_empty() || key.chars().any(char::is_control) => {
                return Err(ProbeIssue::InvalidCredential)
            }
            key => key.as_ref().map(|key| format!("Bearer {key}")),
        };
        // Whatever is left of this poll's time, at most two seconds.
        let budget = super::request_budget(Duration::from_secs(2)).ok_or(ProbeIssue::Timeout)?;
        // No redirects or proxies: credentials stay at the configured endpoint.
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(budget))
            .timeout_connect(Some(budget.min(Duration::from_millis(500))))
            .max_redirects(0)
            .proxy(None)
            .build()
            .into();
        let url = format!("{base}{path}");
        let reply = match json {
            None => {
                let mut request = agent.get(url);
                if let Some(value) = &authorization {
                    request = request.header("Authorization", value.as_str());
                }
                request.call()
            }
            Some(json) => {
                let mut request = agent.post(url).header("Content-Type", "application/json");
                if let Some(value) = &authorization {
                    request = request.header("Authorization", value.as_str());
                }
                request.send(json)
            }
        };
        let mut response = reply.map_err(request_issue)?;
        if response.status() != 200 {
            return Err(ProbeIssue::Http(response.status().as_u16()));
        }
        response
            .body_mut()
            .with_config()
            .limit(MAX_HTTP_RESPONSE_BYTES as u64)
            .read_to_string()
            .map_err(request_issue)
    }
}

fn request_issue(error: ureq::Error) -> ProbeIssue {
    match error {
        ureq::Error::StatusCode(status) => ProbeIssue::Http(status),
        ureq::Error::Timeout(_) => ProbeIssue::Timeout,
        ureq::Error::BodyExceedsLimit(_) => ProbeIssue::ResponseTooLarge,
        _ => ProbeIssue::Transport,
    }
}

fn inventory(provider: &str, names: Vec<String>, kind: &str) -> LlmTelemetry {
    let mut names = names;
    names.sort();
    names.dedup();
    LlmTelemetry {
        source: TelemetrySource::Live,
        observed_at: Some(SystemTime::now()),
        provider: Some(provider.into()),
        status: Some(if names.is_empty() { "no models" } else { kind }.into()),
        model: Some(match names.as_slice() {
            [] => "none".into(),
            [name] => name.clone(),
            _ => format!("{} models · {}", names.len(), names[0]),
        }),
        details: Some(format!("{} {kind} models", names.len())),
        ..LlmTelemetry::default()
    }
}

/// Text a runtime reports at `path`, without control characters; `None`
/// when it is missing or empty.
fn text(value: &Value, path: &[&str]) -> Option<String> {
    let text: String = json_value(value, path)?
        .as_str()?
        .chars()
        .filter(|c| !c.is_control())
        .take(60)
        .collect();
    (!text.trim().is_empty()).then_some(text)
}

/// A positive count at `path` that fits a `u32`.
fn small(value: &Value, path: &[&str]) -> Option<u32> {
    counter(value, path)
        .filter(|n| *n > 0)
        .and_then(|n| u32::try_from(n).ok())
}

pub(super) fn ollama(value: &Value) -> Option<LlmTelemetry> {
    let models = value.get("models")?.as_array()?;
    let names = models
        .iter()
        .map(|m| identifier(m, "name").or_else(|| identifier(m, "model")))
        .collect::<Option<Vec<_>>>()?;
    let mut result = inventory("Ollama", names, "loaded");
    // /api/ps exposes residency and context capacity, not request activity.
    let vram = models
        .iter()
        .try_fold(0_u64, |sum, m| sum.checked_add(counter(m, &["size_vram"])?));
    result.model_memory = vram;
    // `size` is the whole allocation; what is not in `size_vram` runs on CPU.
    let size = models
        .iter()
        .try_fold(0_u64, |sum, m| sum.checked_add(counter(m, &["size"])?));
    if let Some((size, vram)) = size.zip(vram).filter(|(size, _)| *size > 0) {
        result.model_size = Some(size);
        result.model_offloaded = Some(size.saturating_sub(vram));
    }
    let mut details = result.details.take().unwrap();
    if let Some(vram) = vram {
        details.push_str(&format!(" · resident VRAM {}", bytes(vram)));
    }
    if let Some(offloaded) = result.model_offloaded.filter(|bytes| *bytes > 0) {
        details.push_str(&format!(" · {} on CPU", bytes(offloaded)));
    }
    if let [model] = models.as_slice() {
        // What the model is and runs on: its architecture and Ollama's engine.
        let runner = text(model, &["runner"])
            .or_else(|| text(model, &["details", "runner"]))
            .map(|runner| match runner.as_str() {
                "llamacpp" => "llama.cpp".to_owned(),
                "mlx" => "MLX".to_owned(),
                "ggml" => "GGML".to_owned(),
                _ => runner,
            });
        match (text(model, &["details", "family"]), runner) {
            (Some(family), Some(runner)) => details.push_str(&format!(" · {family} on {runner}")),
            (Some(family), None) => details.push_str(&format!(" · {family}")),
            (None, Some(runner)) => details.push_str(&format!(" · on {runner}")),
            (None, None) => {}
        }
        let shown = identifier(model, "digest")
            .and_then(|digest| OLLAMA_SHOWN.with_borrow(|shown| shown.facts(&digest)));
        result.model_facts = ModelFacts {
            quantization: text(model, &["details", "quantization_level"]),
            context_configured: counter(model, &["context_length"]).filter(|n| *n > 0),
            ..shown.unwrap_or_default()
        };
    }
    result.details = Some(details);
    Some(result)
}

/// The list of the models Ollama runs.
const OLLAMA_RUNNING: &str = "/api/ps";

/// What Ollama's `POST /api/show` said about each model it ran, by digest.
///
/// `/api/show` is read-only: Ollama reads the model's manifest and the header
/// of its file, loads nothing and runs no inference. A digest names a model's
/// contents, so what it says never changes: each digest is asked once, and
/// again only a minute after an answer that could not be read. The poll asks
/// for `/api/ps` alone and hands its answer to [`ollama`], so the facts reach
/// it through this table, kept on the sampler thread that polls.
#[derive(Default)]
struct ShownModels(VecDeque<(String, Shown)>);

enum Shown {
    Facts(Box<ModelFacts>),
    /// When an answer could not be read.
    Failed(Instant),
}

thread_local! {
    static OLLAMA_SHOWN: RefCell<ShownModels> = RefCell::default();
}

/// Digests remembered at once: the model running and a few it switched from.
const SHOWN_MODELS: usize = 8;
/// How long a failed `/api/show` waits before it is asked again.
const SHOW_RETRY: Duration = Duration::from_secs(60);

impl ShownModels {
    fn facts(&self, digest: &str) -> Option<ModelFacts> {
        self.0.iter().find_map(|(known, shown)| match shown {
            Shown::Facts(facts) if known == digest => Some(ModelFacts::clone(facts)),
            _ => None,
        })
    }

    /// Whether `/api/show` should be asked about `digest` now.
    fn wants(&self, digest: &str, now: Instant) -> bool {
        match self.0.iter().find(|(known, _)| known == digest) {
            None => true,
            Some((_, Shown::Facts(_))) => false,
            Some((_, Shown::Failed(at))) => now.saturating_duration_since(*at) >= SHOW_RETRY,
        }
    }

    fn record(&mut self, digest: String, facts: Option<ModelFacts>, now: Instant) {
        self.0.retain(|(known, _)| *known != digest);
        if self.0.len() >= SHOWN_MODELS {
            self.0.pop_front();
        }
        let shown = facts.map_or(Shown::Failed(now), |facts| Shown::Facts(Box::new(facts)));
        self.0.push_back((digest, shown));
    }

    /// After `/api/ps` answered `body`: ask `/api/show` about the model Ollama
    /// runs, when it runs one and its digest is new. Facts are only shown for
    /// a lone model, so nothing is asked while several run.
    fn learn(endpoint: &Endpoint, port: u16, body: &str) {
        let Ok(running) = serde_json::from_str::<Value>(body) else {
            return;
        };
        let Some([model]) = running
            .get("models")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
        else {
            return;
        };
        let name = ["name", "model"]
            .iter()
            .find_map(|field| model.get(*field).and_then(Value::as_str))
            .filter(|name| !name.is_empty() && !name.chars().any(char::is_control));
        let (Some(name), Some(digest)) = (name, identifier(model, "digest")) else {
            return;
        };
        let now = Instant::now();
        // Ollama answers for a cloud model by asking its cloud.
        if cloud_model(name) || !OLLAMA_SHOWN.with_borrow(|shown| shown.wants(&digest, now)) {
            return;
        }
        let mut ask = json!({ "model": name });
        // The manifest the runner loaded, for a name with one per runner.
        if let Some(runner) = text(model, &["runner"]) {
            ask["runner"] = Value::String(runner);
        }
        // An answer without facts is still an answer, kept like one with them.
        let facts = endpoint
            .request(port, "/api/show", Some(&ask.to_string()))
            .ok()
            .and_then(|body| serde_json::from_str::<Value>(&body).ok())
            .filter(Value::is_object)
            .map(|show| show_facts(&show).unwrap_or_default());
        diagnostics_log(
            if facts.is_some() { "INFO" } else { "WARN" },
            "ollama_show",
            format!(
                "digest={} read={}",
                log_field(&digest.chars().take(19).collect::<String>()),
                facts.is_some()
            ),
        );
        OLLAMA_SHOWN.with_borrow_mut(|shown| shown.record(digest, facts, now));
    }
}

/// A model Ollama serves from its cloud: named `name:cloud`, or with a tag
/// ending in `-cloud`.
fn cloud_model(name: &str) -> bool {
    name.rsplit_once(':')
        .is_some_and(|(_, tag)| tag == "cloud" || tag.ends_with("-cloud"))
}

/// The keys under a GGUF model's architecture that a model of standard
/// attention carries, besides `rope.*` and `expert_*`. Any other (a sliding
/// window, latent or linear attention, a state space, layers sharing KV)
/// means some layers keep a cache the plain formula does not size.
const PLAIN_ATTENTION: [&str; 23] = [
    "context_length",
    "embedding_length",
    "block_count",
    "feed_forward_length",
    "vocab_size",
    "use_parallel_residual",
    "logit_scale",
    "residual_scale",
    "embedding_scale",
    "pooling_type",
    "attn_logit_softcapping",
    "final_logit_softcapping",
    "leading_dense_block_count",
    "attention.head_count",
    "attention.head_count_kv",
    "attention.key_length",
    "attention.value_length",
    "attention.layer_norm_epsilon",
    "attention.layer_norm_rms_epsilon",
    "attention.causal",
    "attention.clamp_kqv",
    "attention.max_alibi_bias",
    "attention.scale",
];

/// What `/api/show` says about a model's shape: `model_info` holds a GGUF
/// file's own metadata, or for a safetensors model what Ollama reads from its
/// `config.json`. `None` when the answer has no `model_info`.
fn show_facts(show: &Value) -> Option<ModelFacts> {
    let info = show.get("model_info")?.as_object()?;
    let gguf = text(show, &["details", "format"]).as_deref() == Some("gguf");
    let architecture = info
        .get("general.architecture")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let prefix = format!("{architecture}.");
    let own = |key: &str| info.get(&format!("{prefix}{key}"));
    let number = |key: &str| own(key).and_then(Value::as_u64).filter(|n| *n > 0);
    let small = |key: &str| number(key).and_then(|n| u32::try_from(n).ok());
    let mut keys = info.keys().filter_map(|key| key.strip_prefix(&prefix));
    // Latent attention (MLA) caches what the runtime chooses, not heads.
    let latent = keys
        .clone()
        .any(|key| key.starts_with("attention.kv_lora_rank"));
    // Only a GGUF file names every kind of layer; Ollama's reading of a
    // safetensors model's config.json leaves hybrid layers out.
    let plain = gguf
        && !architecture.is_empty()
        && keys.all(|key| {
            PLAIN_ATTENTION.contains(&key) || key.starts_with("rope.") || key.starts_with("expert_")
        });
    let heads = small("attention.head_count");
    let head_dim = match (
        small("attention.key_length"),
        small("attention.value_length"),
    ) {
        (Some(key), value) if value.is_none_or(|value| value == key) => Some(key),
        // llama.cpp's own default when a GGUF file names no key length.
        (None, None) if gguf => heads
            .zip(small("embedding_length"))
            .and_then(|(heads, width)| (width % heads == 0).then_some(width / heads)),
        _ => None,
    };
    Some(ModelFacts {
        parameters: info
            .get("general.parameter_count")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0),
        context_trained: number("context_length"),
        experts: small("expert_count"),
        experts_per_token: small("expert_used_count"),
        kv_layers: small("block_count").filter(|_| plain),
        // A per-layer list of KV heads is not one number: unknown.
        kv_heads: small("attention.head_count_kv").filter(|_| !latent),
        head_dim: head_dim.filter(|_| !latent),
        ..ModelFacts::default()
    })
}

pub(super) fn lm_studio(value: &Value) -> Option<LlmTelemetry> {
    let models = value.get("models")?.as_array()?;
    let mut names = Vec::new();
    let mut loaded = Vec::new();
    for model in models {
        // Embedding and decision models are not language models; a model
        // without `loaded_instances` is simply not loaded.
        if matches!(
            model.get("type").and_then(Value::as_str),
            Some("embedding" | "decision")
        ) {
            continue;
        }
        let Some(instances) = model.get("loaded_instances").and_then(Value::as_array) else {
            continue;
        };
        for instance in instances {
            names.push(identifier(instance, "id").or_else(|| identifier(model, "key"))?);
            loaded.push((model, instance));
        }
    }
    let mut result = inventory("LM Studio", names, "loaded");
    if let [(model, instance)] = loaded.as_slice() {
        result.model_facts = ModelFacts {
            quantization: text(model, &["quantization", "name"]).or_else(|| {
                json_value(model, &["quantization", "bits_per_weight"])
                    .and_then(Value::as_f64)
                    .filter(|bits| *bits > 0.0)
                    .map(|bits| format!("{bits}-bit"))
            }),
            // LM Studio's `num_experts` is how many experts it runs per
            // token, a load setting (0 leaves the model's own).
            experts_per_token: small(instance, &["config", "num_experts"]),
            context_configured: counter(instance, &["config", "context_length"]).filter(|n| *n > 0),
            context_trained: counter(model, &["max_context_length"]).filter(|n| *n > 0),
            ..ModelFacts::default()
        };
        // The weights on disk (no live memory is reported), and how many
        // requests the instance runs at once.
        let details = result.details.as_mut()?;
        if let Some(format) = text(model, &["format"]) {
            details.push_str(&format!(" · {}", format.to_uppercase()));
        }
        if let Some(size) = counter(model, &["size_bytes"]).filter(|n| *n > 0) {
            details.push_str(&format!(" · weights {}", bytes(size)));
        }
        if let Some(parallel) = counter(instance, &["config", "parallel"]).filter(|n| *n > 0) {
            details.push_str(&format!(" · up to {parallel} requests at once"));
        }
    }
    Some(result)
}

pub(super) fn lm_studio_legacy(value: &Value) -> Option<LlmTelemetry> {
    let models = value.get("data")?.as_array()?;
    let mut names = Vec::new();
    for model in models {
        let state = model.get("state")?.as_str()?;
        if state == "loaded" {
            names.push(identifier(model, "id")?);
        }
    }
    Some(inventory("LM Studio", names, "loaded"))
}

pub(super) fn models(provider: &str, value: &Value) -> Option<LlmTelemetry> {
    let models = value.get("data")?.as_array()?;
    let names = models
        .iter()
        .map(|m| identifier(m, "id"))
        .collect::<Option<Vec<_>>>()?;
    // OpenAI's model catalogue is availability, not proof of RAM residency.
    Some(inventory(provider, names, "available"))
}

pub(super) fn seconds_to_ms(seconds: f64) -> Option<u64> {
    let millis = seconds * 1000.0;
    (millis.is_finite() && millis >= 0.0 && millis < u64::MAX as f64).then_some(millis as u64)
}

#[derive(Default)]
struct Metrics {
    // Canonical label maps retain every series, including engine/rank identity.
    values: BTreeMap<String, BTreeMap<BTreeMap<String, String>, f64>>,
}

impl Metrics {
    fn parse(text: &str) -> Self {
        let mut result = Self::default();
        for line in text.lines().map(str::trim).filter(|s| !s.starts_with('#')) {
            let Some(end) = line.find(['{', ' ', '\t']) else {
                continue;
            };
            let name = &line[..end];
            let mut tail = &line[end..];
            let mut labels = BTreeMap::new();
            let mut valid = true;
            if tail.starts_with('{') {
                let mut closed = false;
                tail = &tail[1..];
                loop {
                    tail = tail.trim_start();
                    if let Some(rest) = tail.strip_prefix('}') {
                        tail = rest;
                        closed = true;
                        break;
                    }
                    let Some((key, rest)) = tail.split_once('=') else {
                        break;
                    };
                    let rest = rest.trim_start();
                    if !rest.starts_with('"') {
                        break;
                    }
                    let mut end = 1;
                    let mut escaped = false;
                    for ch in rest[1..].chars() {
                        end += ch.len_utf8();
                        if ch == '"' && !escaped {
                            break;
                        }
                        escaped = ch == '\\' && !escaped;
                    }
                    let Ok(value) = serde_json::from_str::<String>(&rest[..end]) else {
                        break;
                    };
                    if labels.insert(key.trim().to_owned(), value).is_some() {
                        valid = false;
                    }
                    tail = rest[end..].trim_start();
                    if let Some(rest) = tail.strip_prefix(',') {
                        tail = rest;
                    } else if !tail.starts_with('}') {
                        break;
                    }
                }
                valid &= closed && tail.starts_with(char::is_whitespace);
            }
            let value = tail
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<f64>().ok());
            // A malformed series invalidates this metric; never return a partial sum.
            let value = value
                .filter(|v| valid && v.is_finite() && *v >= 0.0)
                .unwrap_or(f64::NAN);
            let series = result.values.entry(name.to_owned()).or_default();
            if series.insert(labels.clone(), value).is_some() {
                series.insert(labels, f64::NAN);
            }
        }
        result
    }

    fn sum(&self, name: &str) -> Option<f64> {
        let sum: f64 = self.values.get(name)?.values().sum();
        sum.is_finite().then_some(sum)
    }

    fn has_prefix(&self, prefix: &str) -> bool {
        self.values.keys().any(|name| name.starts_with(prefix))
    }

    fn count(&self, name: &str) -> Option<u64> {
        self.sum(name)
            .filter(|v| v.fract() == 0.0 && *v < u64::MAX as f64)
            .map(|v| v as u64)
    }

    fn max_fraction(&self, name: &str) -> Option<f64> {
        self.values
            .get(name)?
            .values()
            .try_fold(0.0_f64, |max, value| {
                (value.is_finite() && (0.0..=1.0).contains(value)).then_some(max.max(*value))
            })
    }

    fn delta(&self, previous: &Self, name: &str) -> Option<f64> {
        self.delta_by(previous, name, |_| true)
    }

    /// How much the series of `name` that `keep` selects grew since
    /// `previous`: `None` when one of them fell (a restart), the set of
    /// series changed, or none is selected.
    fn delta_by(
        &self,
        previous: &Self,
        name: &str,
        keep: impl Fn(&BTreeMap<String, String>) -> bool,
    ) -> Option<f64> {
        let current: Vec<_> = self
            .values
            .get(name)?
            .iter()
            .filter(|(labels, _)| keep(labels))
            .collect();
        let old: BTreeMap<_, _> = previous
            .values
            .get(name)?
            .iter()
            .filter(|(labels, _)| keep(labels))
            .collect();
        if current.is_empty() || current.len() != old.len() {
            return None;
        }
        current.into_iter().try_fold(0.0, |sum, (key, value)| {
            let delta = value - *old.get(key)?;
            (delta.is_finite() && delta >= 0.0).then_some(sum + delta)
        })
    }

    /// The sum of the series of `name` that `keep` selects; `None` when it
    /// selects none.
    fn sum_by(&self, name: &str, keep: impl Fn(&BTreeMap<String, String>) -> bool) -> Option<f64> {
        let selected: Vec<f64> = self
            .values
            .get(name)?
            .iter()
            .filter(|(labels, _)| keep(labels))
            .map(|(_, value)| *value)
            .collect();
        let sum: f64 = selected.iter().sum();
        (!selected.is_empty() && sum.is_finite()).then_some(sum)
    }

    /// The mean of what a histogram observed since `previous`: the growth of
    /// its `_sum` over the growth of its `_count`. `None` when it observed
    /// nothing or was reset.
    fn mean_since(&self, previous: &Self, histogram: &str) -> Option<f64> {
        let count = self.delta(previous, &format!("{histogram}_count"))?;
        let sum = self.delta(previous, &format!("{histogram}_sum"))?;
        (count >= 1.0)
            .then_some(sum / count)
            .filter(|mean| mean.is_finite())
    }

    /// The one value every series of `name` reports: a constant each rank
    /// of the server repeats.
    fn constant(&self, name: &str) -> Option<f64> {
        let mut values = self.values.get(name)?.values();
        let first = *values.next()?;
        (first.is_finite() && values.all(|value| *value == first)).then_some(first)
    }
}

pub(super) fn metric_sum(text: &str, name: &str) -> Option<f64> {
    Metrics::parse(text).sum(name)
}

/// The prefill in flight: where its gauge started, where it last advanced, and where it is now.
struct PrefillRun {
    start: (Instant, f64),
    advanced: (Instant, f64),
    live: f64,
}

/// The native runtime's prefill speed, for mlx-serve and sushi.
#[derive(Default)]
struct PrefillWindow {
    run: Option<PrefillRun>,
    polled: bool,
    /// Prompt tokens and prefill seconds of finished requests. Both move only when a request ends.
    finished: Option<(f64, f64)>,
    /// The latest measured speed and when it was taken.
    last: Option<(f64, Instant)>,
}

impl PrefillWindow {
    /// Prefill tokens/s and whether it is live.
    ///
    /// The live gauge advances in chunks of thousands of tokens, so a fixed window reads 0 or twice the
    /// true speed. Instead, while a prefill is in flight, divide its growth since the prefill was first
    /// seen by the time from then to the gauge's latest advance: that reads the true speed between
    /// chunks, and nothing until the first chunk lands. Once nothing is prefilling, the speed of the
    /// requests that finished since the last poll is shown, not live, and it stays until a newer prefill
    /// replaces it; a decode running for minutes does not erase it. Otherwise, before any request has
    /// finished, report a live 0.
    fn rate(&mut self, now: Instant, metrics: &Metrics, series: &Series) -> Option<(f64, bool)> {
        let live = metrics.sum(&series.prompt)?;
        // A server without the flag is prefilling whenever the gauge is above 0.
        let prefilling = metrics
            .sum(&series.requests_prefilling)
            .map_or(live > 0.0, |n| n > 0.0);
        let finished = metrics
            .sum(&series.prefill_total)
            .zip(metrics.sum("vllm:request_prefill_time_seconds_sum"));
        let polled = std::mem::replace(&mut self.polled, true);
        let counters = std::mem::replace(&mut self.finished, finished);
        if prefilling {
            return self.in_flight(now, live);
        }
        self.run = None;
        // A fresh dashboard has no earlier sample to subtract. Show the server's cumulative
        // finished-request prefill speed instead of a blank until the next request ends.
        if !polled {
            if let Some((tokens, seconds)) = finished {
                if tokens > 0.0 && seconds > 0.0 {
                    self.last = Some((tokens / seconds, now));
                }
            }
        }
        if let Some(((tokens, seconds), (old_tokens, old_seconds))) = finished.zip(counters) {
            if tokens > old_tokens && seconds > old_seconds {
                self.last = Some(((tokens - old_tokens) / (seconds - old_seconds), now));
            }
        }
        if let Some((rate, _)) = self.last {
            return Some((rate, false));
        }
        polled.then_some((0.0, true))
    }

    fn in_flight(&mut self, now: Instant, live: f64) -> Option<(f64, bool)> {
        // The gauge falling means that prefill ended and another began.
        let mut run = self
            .run
            .take()
            .filter(|run| live >= run.live)
            .unwrap_or(PrefillRun {
                start: (now, live),
                advanced: (now, live),
                live,
            });
        if live > run.live {
            run.advanced = (now, live);
        }
        run.live = live;
        let ((start, from), (advanced, to)) = (run.start, run.advanced);
        let seconds = advanced.saturating_duration_since(start).as_secs_f64();
        self.run = Some(run);
        let rate = (to - from) / seconds;
        rate.is_finite().then(|| {
            self.last = Some((rate, now));
            (rate, true)
        })
    }
}

/// How long a decode speed measured from the last gauge step stays shown once the gauge stops moving.
const GENERATION_HOLD: Duration = Duration::from_secs(4);

/// How far back a decode speed averages. Long enough to span several server refreshes, short enough
/// that a request that slows down shows it.
const GENERATION_WINDOW: Duration = Duration::from_secs(6);

/// mlx-serve's decode speed.
///
/// `generation_tokens_live` is a gauge the server refreshes once every two seconds, so a delta between
/// two one-second polls reads nothing or twice the true speed depending on where the polls land.
/// Keep the points where the gauge moved instead, and divide across a span several steps wide: the
/// gauge moves by exactly the tokens generated between two refreshes, whatever the poll rate.
#[derive(Default)]
struct GenerationWindow {
    /// Each point where the gauge advanced: when, and by how much in total.
    advances: VecDeque<(Instant, f64)>,
    /// The latest measured speed and when it was taken.
    last: Option<(f64, Instant)>,
    /// Whether any poll has been taken yet.
    polled: bool,
}

impl GenerationWindow {
    /// Whether this is the first poll this process takes. A one-shot report keeps the plain interval
    /// delta for that first poll, because the window needs a second sample before it can read a step.
    fn first_poll(&mut self) -> bool {
        !std::mem::replace(&mut self.polled, true)
    }

    /// Decode tokens/s and whether it is live.
    ///
    /// The gauge itself says whether the server is generating: a request that starts, stalls or ends
    /// simply stops moving it. `num_requests_running` is not used, because it reads 0 between a
    /// request's phases while in-flight tokens are still held, which would restart the window on
    /// every hand-off. Nothing is reported until the first gauge step lands: before that there is no
    /// measurement, and a zero would claim a stalled server.
    fn rate(&mut self, now: Instant, metrics: &Metrics, series: &Series) -> Option<(f64, bool)> {
        self.observe(now, metrics.sum(&series.generation)?)
    }

    /// The same reading for any count of generated tokens: the server-wide gauge, or one session's
    /// `generated_tokens`, which may step on the same refresh.
    fn observe(&mut self, now: Instant, live: f64) -> Option<(f64, bool)> {
        // The gauge falling means a restart or a fresh window; its speed cannot be carried over.
        if self.advances.back().is_some_and(|(_, seen)| *seen > live) {
            self.advances.clear();
            self.last = None;
        }
        if self.advances.back().is_none_or(|(_, seen)| live > *seen) {
            self.advances.push_back((now, live));
        }
        let (advanced, to) = *self.advances.back()?;
        if now.saturating_duration_since(advanced) > GENERATION_HOLD {
            // Nothing has been generated for a hold: the server is idle, stalled or between requests.
            // The next step starts a fresh window rather than carrying this gap into its average.
            self.advances.clear();
            self.last = Some((0.0, now));
            return Some((0.0, true));
        }
        // The newest point at least one window behind the last advance, so the span covers whole steps.
        let (start, from) = self
            .advances
            .iter()
            .rev()
            .find(|(at, _)| advanced.saturating_duration_since(*at) >= GENERATION_WINDOW)
            .or_else(|| self.advances.front())
            .copied()
            .unwrap_or((advanced, to));
        let horizon = GENERATION_WINDOW * 3;
        while self
            .advances
            .front()
            .is_some_and(|(at, _)| advanced.saturating_duration_since(*at) > horizon)
        {
            self.advances.pop_front();
        }
        let seconds = advanced.saturating_duration_since(start).as_secs_f64();
        let rate = (to - from) / seconds;
        if rate.is_finite() && rate > 0.0 {
            self.last = Some((rate, now));
            return Some((rate, true));
        }
        self.hold(now)
    }

    /// The last real speed, shown as an average only while its own step is recent.
    fn hold(&self, now: Instant) -> Option<(f64, bool)> {
        self.last
            .filter(|(rate, at)| {
                *rate > 0.0 && now.saturating_duration_since(*at) <= GENERATION_HOLD
            })
            .map(|(rate, _)| (rate, false))
    }

    fn reset(&mut self) {
        // The poll history stays warm: only the measured run is discarded.
        self.advances.clear();
        self.last = None;
    }
}

/// Each running mlx-serve request's own decode speed, by request id: the prompt it was first seen with,
/// and a stepping window over its `generated_tokens`.
#[derive(Default)]
pub(super) struct SessionSpeeds(BTreeMap<String, (u64, GenerationWindow)>);

#[derive(Default)]
pub(super) struct MetricsHistory {
    previous: Option<(String, Instant, Metrics)>,
    prefill: PrefillWindow,
    sessions: SessionSpeeds,
    generation: GenerationWindow,
    /// Start time of the mlx-serve process that answered `/metrics.json` with a 404.
    no_sessions: Option<f64>,
    /// Where SGLang's schedulers spent the last half-minute.
    scheduler: SchedulerTime,
}

/// The metric names one engine publishes, so `observe` has no per-engine branches.
struct Series {
    prefix: &'static str,
    active: &'static str,
    waiting: &'static str,
    kv: &'static str,
    generation: String,
    prompt: String,
    hits: String,
    queries: String,
    /// The rest of the native runtime's own series. Empty for vLLM and SGLang, whose
    /// rates come from the counters above.
    prefill_expected: String,
    prefill_total: String,
    requests_prefilling: String,
    mlx_active_bytes: String,
    /// Strata's own live-rate gauges, present beside its vLLM-named counters.
    live_generation: String,
    live_prefill: String,
}

fn series(provider: &str, metrics: &Metrics) -> Option<Series> {
    // mlx-serve publishes vLLM-named queues and its own series beside them. Sushi
    // serves the same gauges under `sushi:` instead of `mlx_serve:`.
    let (prefix, active, waiting, kv, native) = match provider {
        "TensorFold" => (
            "tensorfold",
            "requests_running",
            "requests_waiting",
            "kv_cache_usage_ratio",
            None,
        ),
        // Strata serves vLLM's gauge names on /metrics?format=prometheus and adds
        // its own live-rate gauges under `strata:`.
        "Strata" => (
            "vllm",
            "num_requests_running",
            "num_requests_waiting",
            "kv_cache_usage_perc",
            None,
        ),
        "vLLM" => (
            "vllm",
            "num_requests_running",
            "num_requests_waiting",
            "kv_cache_usage_perc",
            None,
        ),
        "mlx-serve" => (
            "vllm",
            "num_requests_running",
            "num_requests_waiting",
            "kv_cache_usage_perc",
            Some(
                if metrics.has_prefix("sushi:") && !metrics.has_prefix("mlx_serve:") {
                    "sushi"
                } else {
                    "mlx_serve"
                },
            ),
        ),
        "SGLang" => (
            "sglang",
            "num_running_reqs",
            "num_queue_reqs",
            "token_usage",
            None,
        ),
        _ => return None,
    };
    let key = |suffix: &str| format!("{prefix}:{suffix}");
    let mut series = Series {
        prefix,
        active,
        waiting,
        kv,
        generation: key("generation_tokens_total"),
        prompt: key("prompt_tokens_total"),
        hits: key("prefix_cache_hits_total"),
        queries: key("prefix_cache_queries_total"),
        prefill_expected: String::new(),
        prefill_total: String::new(),
        requests_prefilling: String::new(),
        mlx_active_bytes: String::new(),
        live_generation: String::new(),
        live_prefill: String::new(),
    };
    if provider == "TensorFold" {
        series.hits = key("prompt_tokens_cached_total");
        series.queries = key("prompt_tokens_total");
    }
    if provider == "SGLang" {
        // SGLang counts the prompt tokens served from its cache, not lookups.
        series.hits = key("cached_tokens_total");
        series.queries = key("prompt_tokens_total");
    }
    if provider == "vLLM"
        && !metrics.values.contains_key(&series.queries)
        && metrics
            .values
            .contains_key("vllm:gpu_prefix_cache_queries_total")
    {
        // vLLM 0.9.1 and earlier name the prefix-cache counters `gpu_prefix_cache_*`.
        series.hits = key("gpu_prefix_cache_hits_total");
        series.queries = key("gpu_prefix_cache_queries_total");
    }
    if provider == "Strata" {
        series.live_generation = "strata:live_tok_s".into();
        series.live_prefill = "strata:live_prefill_tok_s_mean".into();
    }
    if let Some(namespace) = native {
        // Tokens generated so far by running requests, tokens prefilled so far by the
        // request in flight, and the tokens restored from the prefix cache (over all
        // prompt tokens). These are the server's own series; it publishes no
        // vLLM-named token counters.
        series.generation = format!("{namespace}:generation_tokens_live");
        series.prompt = format!("{namespace}:prefill_tokens_live");
        series.hits = format!("{namespace}:prefix_cache_tokens_total");
        series.queries = key("prompt_tokens_total");
        series.prefill_expected = format!("{namespace}:prefill_tokens_expected");
        series.prefill_total = format!("{namespace}:prefill_tokens_total");
        series.requests_prefilling = format!("{namespace}:requests_prefilling");
        series.mlx_active_bytes = format!("{namespace}:mlx_active_bytes");
    }
    Some(series)
}

/// The series that gives the server's start time: bare in Prometheus's
/// convention, prefixed by mlx-serve. Sushi and multi-process vLLM have none.
fn start_series(metrics: &Metrics) -> Option<&'static str> {
    [
        "process_start_time_seconds",
        "mlx_serve:process_start_time_seconds",
    ]
    .into_iter()
    .find(|name| metrics.values.contains_key(*name))
}

impl MetricsHistory {
    fn start_time(&self) -> Option<f64> {
        let (_, _, metrics) = self.previous.as_ref()?;
        start_series(metrics).and_then(|name| metrics.sum(name))
    }

    /// A build without `/metrics.json` is asked again only after the server restarts.
    pub fn sessions_known_missing(&self) -> bool {
        self.no_sessions.is_some() && self.no_sessions == self.start_time()
    }

    pub fn note_sessions_missing(&mut self) {
        self.no_sessions = self.start_time();
    }

    /// Fill in mlx-serve's running requests and each request's own decode speed.
    pub(super) fn apply_sessions(
        &mut self,
        result: &mut LlmTelemetry,
        value: &Value,
        now: SystemTime,
        at: Instant,
    ) {
        apply_mlx_serve_sessions(result, value, now, at, &mut self.sessions);
    }

    /// What mlx-serve and sushi add to the shared vLLM-shaped result: MLX memory, the prefill
    /// rate, and the progress of the prefill in flight.
    fn native_extras(
        &mut self,
        metrics: &Metrics,
        series: &Series,
        details: &mut Vec<String>,
        result: &mut LlmTelemetry,
        now: Instant,
    ) {
        let done = metrics.count(&series.prompt);
        let expected = metrics.count(&series.prefill_expected);
        result.prefill_tokens_done = done;
        result.prefill_tokens_expected = expected.filter(|expected| *expected > 0);
        if let Some((done, expected)) = done
            .zip(result.prefill_tokens_expected)
            .map(|(done, expected)| (done as f64, expected as f64))
        {
            details.push(format!(
                "prefill {done:.0} / {expected:.0} tokens ({:.0}%)",
                done / expected * 100.0
            ));
        }
        result.model_memory = metrics.count(&series.mlx_active_bytes);
        // The generation gauge is a staircase the server refreshes every two seconds; the stepping
        // window reads its slope, so the panel never alternates between 0 and twice the true speed.
        let first = self.generation.first_poll();
        match self.generation.rate(now, metrics, series) {
            Some((generation, live)) => {
                result.generation_tps = Some(generation);
                result.generation_tps_live = live;
            }
            // A one-shot report has only its cold-start sample, so it keeps the interval delta it
            // already has rather than claiming nothing was measured.
            None if first => {}
            // A running window with no step yet: nothing is known about this request.
            None => {
                result.generation_tps = None;
                result.generation_tps_live = false;
            }
        }
        // Always assigned: the generic gauge delta must never stand in for the prefill speed.
        let (tps, live) = self.prefill.rate(now, metrics, series).unzip();
        result.prefill_tps = tps;
        result.prefill_tps_live = live.unwrap_or(false);
    }

    pub fn observe(&mut self, provider: &str, text: &str, now: Instant) -> Option<LlmTelemetry> {
        let metrics = Metrics::parse(text);
        let series = series(provider, &metrics)?;
        let prefix = series.prefix;
        let active = metrics.count(&format!("{prefix}:{}", series.active));
        let waiting = metrics.count(&format!("{prefix}:{}", series.waiting));
        if active.is_none()
            && waiting.is_none()
            && metrics.sum(&series.generation).is_none()
            && metrics.sum(&series.prompt).is_none()
        {
            return None;
        }
        let names = metrics
            .values
            .iter()
            .filter(|(name, _)| name.starts_with(&format!("{prefix}:")))
            .flat_map(|(_, series)| {
                series
                    .keys()
                    .filter_map(|labels| labels.get("model_name").cloned())
            })
            .map(|s| s.chars().filter(|c| !c.is_control()).take(120).collect())
            .collect();
        let mut result = inventory(provider, names, "available");
        result.active_requests = active;
        result.waiting_requests = waiting;
        result.status = Some(
            match (active, waiting) {
                (Some(0), Some(0)) => "idle",
                (Some(0), Some(_)) => "waiting",
                (Some(n), _) if n > 0 => "processing",
                _ => "running",
            }
            .into(),
        );
        // Seconds since the scrape before, when its counters can be compared with these.
        let mut since = None;
        if let Some((old_provider, old_time, previous)) = &self.previous {
            let elapsed = now.saturating_duration_since(*old_time).as_secs_f64();
            let start = |metrics: &Metrics| {
                start_series(metrics).and_then(|name| metrics.values.get(name).cloned())
            };
            let same_start = start(&metrics) == start(previous);
            if !same_start {
                // A restarted server's gauges start over, so no earlier speed applies to them.
                self.generation.reset();
                self.sessions = SessionSpeeds::default();
                self.prefill = PrefillWindow::default();
                self.scheduler = SchedulerTime::default();
            }
            let span = super::rate_span(Duration::from_secs(5)).as_secs_f64();
            if old_provider == provider && elapsed > 0.0 && elapsed <= span && same_start {
                since = Some(elapsed);
                result.generation_tps = metrics
                    .delta(previous, &series.generation)
                    .map(|n| n / elapsed);
                result.prefill_tps = metrics.delta(previous, &series.prompt).map(|n| n / elapsed);
                if provider == "vLLM" {
                    // Cached prompt tokens were never computed: leave them out.
                    if let (Some(prefill), Some(cached)) = (
                        result.prefill_tps,
                        metrics.delta(previous, "vllm:prompt_tokens_cached_total"),
                    ) {
                        result.prefill_tps = Some((prefill - cached / elapsed).max(0.0));
                    }
                }
                let queries = metrics.delta(previous, &series.queries);
                let hits = metrics.delta(previous, &series.hits);
                result.cache_interval_efficiency = hits
                    .zip(queries)
                    .filter(|(hits, queries)| *queries > 0.0 && hits <= queries)
                    .map(|(hits, queries)| hits / queries * 100.0);
                result.generation_tps_live = result.generation_tps.is_some();
                result.prefill_tps_live = result.prefill_tps.is_some();
            }
        }
        let mut details = vec!["throughput: server token deltas".to_owned()];
        if let Some(usage) = metrics
            .max_fraction(&format!("{prefix}:{}", series.kv))
            .or_else(|| metrics.max_fraction(&format!("{prefix}:gpu_cache_usage_perc")))
        {
            details.push(format!("KV occupancy max {:.1}%", usage * 100.0));
        }
        let hits = metrics.sum(&series.hits);
        let queries = metrics.sum(&series.queries);
        if let Some((hits, queries)) = hits.zip(queries).filter(|(h, q)| *q > 0.0 && h <= q) {
            result.prefix_hit_rate = Some(hits / queries * 100.0);
            result.cache_efficiency = result.prefix_hit_rate;
        } else if provider == "SGLang" {
            // A single series is unambiguous; averaging ratios across engines isn't.
            if metrics
                .values
                .get(&format!("{prefix}:cache_hit_rate"))
                .is_some_and(|s| s.len() == 1)
            {
                result.prefix_hit_rate = metrics
                    .max_fraction(&format!("{prefix}:cache_hit_rate"))
                    .map(|v| v * 100.0);
            }
        }
        let sum = metrics.sum(&format!("{prefix}:time_to_first_token_seconds_sum"));
        let count = metrics.sum(&format!("{prefix}:time_to_first_token_seconds_count"));
        if let Some((sum, count)) = sum.zip(count).filter(|(_, count)| *count > 0.0) {
            details.push(format!(
                "server mean TTFT {:.0} ms (cumulative)",
                sum / count * 1000.0
            ));
        }
        if provider == "mlx-serve" {
            self.native_extras(&metrics, &series, &mut details, &mut result, now);
        }
        if provider == "vLLM" {
            // A prompt's tokens, cached ones included, are counted in one
            // step: tokens per poll, not a speed. Cached tokens were not read.
            result.prefill_tps_live = false;
        }
        if provider == "TensorFold" {
            // Completion bursts do not measure the speed of an in-flight request.
            result.generation_tps_live = false;
            result.prefill_tps_live = false;
            // Device memory is reported on CUDA only; on a Mac the process footprint
            // holds the model.
            result.model_memory = metrics
                .count("tensorfold:device_memory_bytes")
                .or_else(|| metrics.count("tensorfold:process_footprint_bytes"));
            result.total_prompt_tokens = metrics.count(&series.queries);
            result.total_cached_tokens = metrics.count(&series.hits).filter(|hits| {
                result
                    .total_prompt_tokens
                    .is_some_and(|prompt| *hits <= prompt)
            });
            details[0] = "throughput: completed-request token deltas".into();
        }
        if provider == "SGLang" {
            // SGLang adds a request's tokens to its counters when it finishes.
            result.generation_tps_live = false;
            result.prefill_tps_live = false;
            details[0] = "throughput: completed-request token deltas".into();
        }
        if provider == "Strata" {
            // Strata's counters move when requests finish, but the engine also
            // publishes live-rate gauges. A gauge wins; the completion deltas
            // otherwise stay averages that never stand in for an in-flight request.
            let live_decode = metrics
                .sum(&series.live_generation)
                .filter(|rate| *rate > 0.0);
            let live_prefill = metrics.sum(&series.live_prefill).filter(|rate| *rate > 0.0);
            result.generation_tps = live_decode.or(result.generation_tps);
            result.generation_tps_live = live_decode.is_some();
            result.prefill_tps = live_prefill.or(result.prefill_tps);
            result.prefill_tps_live = live_prefill.is_some();
            result.model_memory = metrics.count("strata:gpu_mem_used_bytes");
            if let Some(read) = metrics
                .sum("strata:live_prompt_read")
                .filter(|read| *read > 0.0)
            {
                details.push(format!("prefill {read:.0} tokens read"));
            }
            details[0] = if live_decode.is_some() || live_prefill.is_some() {
                "throughput: engine live gauges".into()
            } else {
                "throughput: completed-request token deltas".into()
            };
        }
        // What each engine's own counters add: its decode speed while it
        // generates, KV room and preemptions, and what the work since the
        // last poll waited for.
        let previous = since.zip(self.previous.as_ref().map(|(_, _, previous)| previous));
        let interval = previous.map(|(elapsed, previous)| Interval { previous, elapsed });
        match provider {
            "vLLM" => vllm_attribution(&metrics, interval, &mut result, &mut details),
            "SGLang" => sglang_attribution(
                &metrics,
                interval,
                &mut self.scheduler,
                now,
                &mut result,
                &mut details,
            ),
            "TensorFold" => tensorfold_attribution(&metrics, interval, &mut result, &mut details),
            _ => {}
        }
        result.details = Some(details.join(" · "));
        self.previous = Some((provider.to_owned(), now, metrics));
        Some(result)
    }
}

/// The scrape before this one, and the seconds between them, when their
/// counters can be compared: the same server, not restarted, within a span
/// that measures the present.
#[derive(Clone, Copy)]
struct Interval<'a> {
    previous: &'a Metrics,
    elapsed: f64,
}

/// A wait in plain units: `350 ms`, `1.2 s`.
fn wait(seconds: f64) -> String {
    if seconds >= 1.0 {
        format!("{seconds:.1} s")
    } else {
        format!("{:.0} ms", seconds * 1000.0)
    }
}

/// `1 request`, `3 requests`.
fn requests(count: u64) -> String {
    format!("{count} request{}", if count == 1 { "" } else { "s" })
}

/// What the work since the last poll did, as one details part.
fn since_last_poll(parts: Vec<String>) -> Option<String> {
    (!parts.is_empty()).then(|| format!("since the last poll: {}", parts.join(", ")))
}

/// The mean queue wait and gap between tokens of the work done in
/// `interval`, from two histograms' `_sum` and `_count`.
fn waits(metrics: &Metrics, interval: Interval, queue: &str, between_tokens: &str) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(mean) = metrics.mean_since(interval.previous, queue) {
        parts.push(format!("queue wait {}", wait(mean)));
    }
    if let Some(mean) = metrics.mean_since(interval.previous, between_tokens) {
        parts.push(format!("{} between tokens", wait(mean)));
    }
    parts
}

/// The first details part of an engine whose decode speed is live while its
/// prompt counters move only when a request ends.
const LIVE_DECODE: &str = "throughput: live decode tokens, prefill at completion";

/// What vLLM's own series add: requests preempted for KV room, the KV room
/// left and the format the cache is stored in, and for the work since the
/// last poll, the tokens a verify pass emitted with a draft, the share of
/// draft tokens accepted, and the mean queue wait and gap between tokens.
fn vllm_attribution(
    metrics: &Metrics,
    interval: Option<Interval>,
    result: &mut LlmTelemetry,
    details: &mut Vec<String>,
) {
    result.preemptions_total = metrics.count("vllm:num_preemptions_total");
    result.kv_free_tokens = vllm_kv_free_tokens(metrics);
    result.model_facts.kv_dtype = vllm_kv_dtype(metrics);
    // An engine registers its draft counters at start exactly when it has a
    // draft; one without them that names its usage gauge as vLLM's V1 engine
    // does has none.
    result.model_facts.draft = if metrics
        .values
        .contains_key("vllm:spec_decode_num_drafts_total")
    {
        Some(true)
    } else {
        metrics
            .values
            .contains_key("vllm:kv_cache_usage_perc")
            .then_some(false)
    };
    let Some(interval) = interval else {
        return;
    };
    let grown = |name: &str| metrics.delta(interval.previous, name);
    let drafts = grown("vllm:spec_decode_num_drafts_total").filter(|n| *n >= 1.0);
    let drafted = grown("vllm:spec_decode_num_draft_tokens_total").filter(|n| *n >= 1.0);
    // No draft accepts more tokens than it proposed.
    let accepted = grown("vllm:spec_decode_num_accepted_tokens_total")
        .filter(|accepted| drafted.is_none_or(|drafted| *accepted <= drafted));
    // Accepted tokens per draft, plus the one the target model adds: vLLM's
    // mean acceptance length.
    result.tokens_per_pass = drafts
        .zip(accepted)
        .map(|(drafts, accepted)| 1.0 + accepted / drafts);
    let mut parts = waits(
        metrics,
        interval,
        "vllm:request_queue_time_seconds",
        "vllm:inter_token_latency_seconds",
    );
    if let Some((accepted, drafted)) = accepted.zip(drafted) {
        parts.push(format!(
            "{:.0}% of draft tokens accepted",
            accepted / drafted * 100.0
        ));
    }
    details.extend(since_last_poll(parts));
}

/// The KV room vLLM has left, in tokens: each engine's capacity less the
/// share its usage gauge says is held, on the engine with the least room.
/// Capacity is `kv_cache_size_tokens` from `vllm:cache_config_info`, which
/// counts a hybrid model's tokens as the scheduler does, else blocks × block
/// size.
fn vllm_kv_free_tokens(metrics: &Metrics) -> Option<u64> {
    let configs = metrics.values.get("vllm:cache_config_info")?;
    let usage = metrics
        .values
        .get("vllm:kv_cache_usage_perc")
        .or_else(|| metrics.values.get("vllm:gpu_cache_usage_perc"))?;
    let mut least: Option<u64> = None;
    for (labels, used) in usage {
        let config = configs
            .keys()
            .find(|config| config.get("engine") == labels.get("engine"))?;
        let number = |key: &str| config.get(key)?.parse::<u64>().ok().filter(|n| *n > 0);
        let capacity = number("kv_cache_size_tokens")
            .or_else(|| number("num_gpu_blocks")?.checked_mul(number("block_size")?))?;
        if !(0.0..=1.0).contains(used) {
            return None;
        }
        // The gauge is held blocks over all blocks: the product is whole
        // blocks of tokens, give or take the float.
        let room = (capacity as f64 * (1.0 - used)).round() as u64;
        least = Some(least.map_or(room, |least| least.min(room)));
    }
    least
}

/// The format vLLM keeps its KV cache in, when set (`auto` is the model's
/// own dtype, which these series do not name).
fn vllm_kv_dtype(metrics: &Metrics) -> Option<String> {
    let dtypes: BTreeSet<&str> = metrics
        .values
        .get("vllm:cache_config_info")?
        .keys()
        .filter_map(|labels| labels.get("cache_dtype").map(String::as_str))
        .collect();
    match dtypes.into_iter().collect::<Vec<_>>().as_slice() {
        [dtype] if *dtype != "auto" && !dtype.is_empty() => {
            Some(dtype.chars().filter(|c| !c.is_control()).take(24).collect())
        }
        _ => None,
    }
}

/// Bytes in SGLang's "GB", which is 2³⁰ bytes.
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// SGLang's decode-token counter, which it advances on every decode step.
const SGLANG_TOKENS: &str = "sglang:realtime_tokens_total";

/// What SGLang's own series add: a live decode speed, requests retracted
/// (its preemptions) for KV room, the KV room left in tokens and bytes, what
/// a token of KV costs, the context it allows and the memory its weights
/// took, and for the work since the last poll the mean queue wait, the gap
/// between tokens and the cached tokens it evicted, with where its schedulers
/// spent the last half-minute.
fn sglang_attribution(
    metrics: &Metrics,
    interval: Option<Interval>,
    scheduler: &mut SchedulerTime,
    now: Instant,
    result: &mut LlmTelemetry,
    details: &mut Vec<String>,
) {
    result.preemptions_total = metrics.count("sglang:num_retracted_requests_total");
    result.kv_free_tokens = sglang_kv_free_tokens(metrics);
    let per_token = sglang_kv_bytes_per_token(metrics);
    result.kv_free_bytes = result
        .kv_free_tokens
        .zip(per_token)
        .and_then(|(tokens, bytes)| tokens.checked_mul(bytes));
    let facts = &mut result.model_facts;
    facts.kv_bytes_per_token_reported = per_token;
    facts.context_configured = metrics
        .constant("sglang:context_len")
        .filter(|tokens| *tokens >= 1.0)
        .map(|tokens| tokens as u64);
    facts.loaded_bytes = metrics
        .constant("sglang:weight_memory_usage_gb")
        .filter(|gb| *gb > 0.0)
        .map(|gb| (gb * GIB).round() as u64);
    let decoding = |labels: &BTreeMap<String, String>| {
        labels.get("mode").map(String::as_str) == Some("decode")
    };
    let counter = metrics.sum_by(SGLANG_TOKENS, decoding).is_some();
    // An older SGLang has only the speed of its last few dozen decode steps,
    // which it keeps for half a minute after decoding stops.
    let gauge = metrics.sum("sglang:gen_throughput").filter(|_| !counter);
    if counter || gauge.is_some() {
        let running = result.active_requests.is_some_and(|n| n > 0);
        let decoded = match gauge {
            // Tokens decoded between two polls: live, and zero through a stall.
            None => interval.and_then(|interval| {
                Some(
                    metrics.delta_by(interval.previous, SGLANG_TOKENS, decoding)?
                        / interval.elapsed,
                )
            }),
            Some(rate) => Some(if running { rate } else { 0.0 }),
        };
        result.generation_tps = decoded;
        result.generation_tps_live = decoded.is_some();
        details[0] = match gauge {
            None => LIVE_DECODE.into(),
            Some(_) => "throughput: engine decode gauge, prefill at completion".into(),
        };
    }
    // Accepted draft tokens plus the bonus token per verify step, over the
    // last few dozen steps: only while it is decoding.
    let lengths: Vec<f64> = metrics
        .values
        .get("sglang:spec_accept_length")
        .into_iter()
        .flat_map(BTreeMap::values)
        .copied()
        .filter(|length| *length >= 1.0)
        .collect();
    if result.generation_tps_live && result.generation_tps.is_some_and(|rate| rate > 0.0) {
        result.tokens_per_pass =
            (!lengths.is_empty()).then(|| lengths.iter().sum::<f64>() / lengths.len() as f64);
    }
    // SGLang names its active draft's size once it has decoded. Without a
    // draft it reads 0, as a server that has not decoded yet does, so only a
    // draft is known.
    if !lengths.is_empty()
        || metrics
            .sum("sglang:spec_num_draft_tokens")
            .is_some_and(|tokens| tokens > 0.0)
    {
        result.model_facts.draft = Some(true);
    }
    if let Some(interval) = interval {
        let mut parts = waits(
            metrics,
            interval,
            "sglang:queue_time_seconds",
            "sglang:inter_token_latency_seconds",
        );
        if let Some(evicted) = metrics
            .delta(interval.previous, "sglang:evicted_tokens_total")
            .filter(|tokens| *tokens >= 1.0)
        {
            parts.push(format!(
                "{} cached tokens evicted",
                compact_tokens(evicted as u64)
            ));
        }
        details.extend(since_last_poll(parts));
        scheduler.observe(now, interval.elapsed, scheduler_shares(metrics, interval));
    }
    details.extend(scheduler.text(now));
}

/// The KV room SGLang has left, in tokens: free slots and slots holding
/// cached prefixes it would evict for a new request, on the rank with the
/// least.
fn sglang_kv_free_tokens(metrics: &Metrics) -> Option<u64> {
    let available = metrics.values.get("sglang:kv_available_tokens")?;
    let evictable = metrics.values.get("sglang:kv_evictable_tokens")?;
    let mut least: Option<f64> = None;
    for (labels, free) in available {
        let room = free + evictable.get(labels)?;
        if !room.is_finite() {
            return None;
        }
        least = Some(least.map_or(room, |least| least.min(room)));
    }
    least.map(|room| room as u64)
}

/// What a token of KV costs on SGLang: the bytes of its KV pool over the
/// tokens it holds. A hybrid model's sliding-window or Mamba pool shares the
/// byte figure, so it gives none.
fn sglang_kv_bytes_per_token(metrics: &Metrics) -> Option<u64> {
    let hybrid = metrics
        .values
        .contains_key("sglang:max_total_num_tokens_swa")
        || [
            "sglang:swa_available_tokens",
            "sglang:swa_used_tokens",
            "sglang:mamba_available_tokens",
            "sglang:mamba_used_tokens",
        ]
        .iter()
        .any(|name| metrics.sum(name).is_some_and(|tokens| tokens > 0.0));
    let pool = metrics.constant("sglang:kv_cache_memory_usage_gb")?;
    let tokens = metrics.constant("sglang:max_total_num_tokens")?;
    (!hybrid && pool > 0.0 && tokens >= 1.0).then(|| (pool * GIB / tokens).round() as u64)
}

/// How far back the scheduler's time split looks.
const SCHEDULER_WINDOW: Duration = Duration::from_secs(30);

/// Where SGLang's schedulers spent the last half-minute, in seconds per
/// second of wall time for each scheduler: the GPU running forward passes
/// (timed only with `SGLANG_ENABLE_METRICS_DEVICE_TIMER`), the scheduler with
/// no work at all, and the scheduler's CPU time across its threads. SGLang
/// adds to these counters about once a second, so one poll may be credited
/// two seconds and the next none; the window evens that out.
#[derive(Default)]
struct SchedulerTime(VecDeque<(Instant, f64, [Option<f64>; 3])>);

impl SchedulerTime {
    fn observe(&mut self, now: Instant, wall: f64, shares: [Option<f64>; 3]) {
        self.0.push_back((now, wall, shares));
    }

    /// The window's shares, as a details part, when any is known.
    fn text(&mut self, now: Instant) -> Option<String> {
        while self
            .0
            .front()
            .is_some_and(|(at, ..)| now.saturating_duration_since(*at) > SCHEDULER_WINDOW)
        {
            self.0.pop_front();
        }
        let wall: f64 = self.0.iter().map(|(_, wall, _)| wall).sum();
        let parts: Vec<String> = ["GPU busy", "scheduler idle", "scheduler CPU"]
            .into_iter()
            .enumerate()
            .filter_map(|(part, name)| {
                let (mut seconds, mut covered) = (0.0, 0.0);
                for (_, wall, shares) in &self.0 {
                    if let Some(share) = shares[part] {
                        seconds += share * wall;
                        covered += wall;
                    }
                }
                let share = (covered > 0.0).then_some(seconds / covered)?;
                // Busy and idle are parts of the wall; CPU spans threads.
                let share = if part < 2 { share.min(1.0) } else { share };
                Some(format!("{name} {:.0}%", share * 100.0))
            })
            .collect();
        (!parts.is_empty()).then(|| format!("last {:.0} s: {}", wall, parts.join(", ")))
    }
}

/// The shares of `interval` each scheduler spent with the GPU busy, idle and
/// on the CPU. A share past what one second can hold is a lump SGLang
/// credited late (idle time it adds when work resumes), not this poll's.
fn scheduler_shares(metrics: &Metrics, interval: Interval) -> [Option<f64>; 3] {
    let per_scheduler = |name: &str, most: f64| -> Option<f64> {
        // A scheduler is a set of labels, whatever the category it times.
        let schedulers = metrics
            .values
            .get(name)?
            .keys()
            .map(|labels| {
                let mut labels = labels.clone();
                labels.remove("category");
                labels
            })
            .collect::<BTreeSet<_>>()
            .len();
        let seconds = metrics.delta(interval.previous, name)?;
        let share = seconds / schedulers as f64 / interval.elapsed;
        (share <= most).then_some(share)
    };
    [
        per_scheduler("sglang:forward_execution_seconds_total", 1.25),
        per_scheduler("sglang:scheduler_idle_seconds_total", 1.25),
        per_scheduler("sglang:scheduler_process_cpu_seconds_total", 16.0),
    ]
}

/// TensorFold's tokens held by running streams, which pass to its finished
/// total when a request ends.
const TENSORFOLD_RUNNING: &str = "tensorfold:generation_tokens_running";

/// Finished tokens that show TensorFold drafts nothing: enough for several
/// decode rounds, each of which would have drafted with MTP on.
const UNDRAFTED_TOKENS: u64 = 32;

/// What TensorFold's own series add: a live decode speed from the tokens its
/// running streams hold, whether it drafts with MTP and the tokens a decode
/// round of the requests finished since the last poll emitted, the requests
/// that gave a lane up to a later one, and the requests its admission
/// refused.
fn tensorfold_attribution(
    metrics: &Metrics,
    interval: Option<Interval>,
    result: &mut LlmTelemetry,
    details: &mut Vec<String>,
) {
    result.preemptions_total = metrics.count("tensorfold:preemptions_total");
    let drafted = metrics.count("tensorfold:mtp_drafted_total");
    let finished = metrics.count("tensorfold:generation_tokens_total");
    result.model_facts.draft = match (drafted, finished) {
        (Some(drafted), _) if drafted > 0 => Some(true),
        (Some(_), Some(finished)) if finished >= UNDRAFTED_TOKENS => Some(false),
        _ => None,
    };
    // A round emits the target's own token and the drafted tokens it kept.
    result.tokens_per_pass = interval.and_then(|interval| {
        let grown = |name: &str| metrics.delta(interval.previous, name);
        let rounds = grown("tensorfold:decode_rounds_total").filter(|n| *n >= 1.0)?;
        let drafted = grown("tensorfold:mtp_drafted_total").filter(|n| *n >= 1.0)?;
        let accepted = grown("tensorfold:mtp_accepted_total").filter(|kept| *kept <= drafted)?;
        Some(1.0 + accepted / rounds)
    });
    if metrics.values.contains_key(TENSORFOLD_RUNNING) {
        // Running plus finished tokens grow by what was decoded, however
        // many requests end between two polls.
        let generated = |metrics: &Metrics| {
            Some(
                metrics.sum(TENSORFOLD_RUNNING)?
                    + metrics.sum("tensorfold:generation_tokens_total")?,
            )
        };
        let decoded = interval.and_then(|interval| {
            let grown = generated(metrics)? - generated(interval.previous)?;
            (grown.is_finite() && grown >= 0.0).then(|| grown / interval.elapsed)
        });
        result.generation_tps = decoded;
        result.generation_tps_live = decoded.is_some();
        details[0] = LIVE_DECODE.into();
    }
    if let Some(refused) = metrics
        .count("tensorfold:admission_rejected_total")
        .filter(|n| *n > 0)
    {
        details.push(format!(
            "{} refused admission since the server started",
            requests(refused)
        ));
    }
}

/// Fill in the model and the live requests from mlx-serve's `/metrics.json` sessions. Sessions that
/// only hold a prefix cache are not requests and are skipped.
///
/// A session's `generated_tokens` is the tokens its client receives, so its growth is that request's
/// own speed, not the server total the generation panel shows. It is read through the same stepping
/// window as the server gauge, so it holds steady whether the count moves every decode tick or only
/// when the server refreshes its gauges.
pub(super) fn apply_mlx_serve_sessions(
    result: &mut LlmTelemetry,
    value: &Value,
    now: SystemTime,
    at: Instant,
    speeds: &mut SessionSpeeds,
) {
    let Some(sessions) = value.get("sessions").and_then(Value::as_array) else {
        return;
    };
    if let Some(model) = sessions.iter().find_map(|s| identifier(s, "model")) {
        result.model = Some(model);
    }
    let mut seen = BTreeMap::new();
    result.requests = sessions
        .iter()
        .filter(|s| matches!(s["phase"].as_str(), Some("prefill" | "decode")))
        .enumerate()
        .filter_map(|(index, s)| {
            let context = counter(s, &["context_tokens"])?;
            let output = counter(s, &["generated_tokens"])?;
            let prompt = context.saturating_sub(output);
            // The server's id 0 means none. A synthetic slot name is positional, so it may move to a
            // different request between samples and never carries a speed.
            let real = identifier(s, "request_id").filter(|id| id != "0");
            let id = real.clone().unwrap_or_else(|| format!("slot-{index}"));
            let output_tps = real.and_then(|real| {
                // The prompt is the request's identity: an id reused with another prompt starts over.
                let (_, mut window) = speeds
                    .0
                    .remove(&real)
                    .filter(|(seen, _)| *seen == prompt)
                    .unwrap_or_default();
                // Prefill sessions move `generated_tokens` by zero; a speed there is not a decode rate.
                let tps = (s["phase"].as_str() == Some("decode"))
                    .then(|| window.observe(at, output as f64))
                    .flatten()
                    .map(|(tps, _)| tps);
                seen.insert(real, (prompt, window));
                tps
            });
            Some(RequestUsage {
                provider: "mlx-serve".into(),
                model: identifier(s, "model").unwrap_or_default(),
                id,
                prompt,
                cached: counter(s, &["cached_tokens"]),
                output: Some(output),
                output_tps,
                completed: false,
                queued: false,
                prefilled: None,
                idle_for: None,
                ttft_ms: None,
                observed_at: Some(now),
            })
        })
        .collect();
    // Only the requests still running are kept, so an id the server reuses after a restart never
    // inherits an old count.
    speeds.0 = seen;
    // vLLM's cache counters only move when a request completes, so a decode can leave the interval
    // rate blank for minutes. The sessions carry the prefix reuse of the requests still running;
    // use it when no completed request supplied an interval rate.
    if result.cache_interval_efficiency.is_none() {
        let totals = result
            .requests
            .iter()
            .try_fold((0_u64, 0_u64), |(cached, prompt), r| {
                Some((
                    cached.checked_add(r.cached?)?,
                    prompt.checked_add(r.prompt)?,
                ))
            });
        if let Some((cached, prompt)) =
            totals.filter(|(cached, prompt)| *prompt > 0 && cached <= prompt)
        {
            result.cache_interval_efficiency = Some(cached as f64 / prompt as f64 * 100.0);
        }
    }
    if let [only] = result.requests.as_slice() {
        result.prompt_tokens = Some(only.prompt);
        result.output_tokens = only.output;
    }
}

impl Sample {
    /// What vLLM's, SGLang's and TensorFold's own counters say about
    /// speculative decoding and the KV cache, for Diagnostics: each reading
    /// only when the runtime reports it. Other runtimes give their own.
    pub(crate) fn native_runtime_lines(&self) -> Vec<String> {
        // SGLang calls its preemptions retractions.
        let preempted = match self.llm_provider.as_str() {
            "vLLM" | "TensorFold" => "preempted",
            "SGLang" => "retracted",
            _ => return Vec::new(),
        };
        let mut lines = Vec::new();
        if let Some(tokens) = self.llm_tokens_per_pass {
            lines.push(format!(
                "Speculative decoding: {tokens:.1} tokens per verify pass"
            ));
        }
        let mut cache = Vec::new();
        if let Some(free) = self.llm_kv_free_tokens {
            let size = self
                .llm_kv_free_bytes
                .map(|free| format!(" ({})", bytes(free)))
                .unwrap_or_default();
            cache.push(format!("{} tokens free{size}", compact_tokens(free)));
        }
        if let Some(count) = self.llm_preemptions_total.filter(|count| *count > 0) {
            cache.push(format!(
                "{} {preempted} since the server started",
                requests(count)
            ));
        }
        if !cache.is_empty() {
            lines.push(format!("KV cache: {}", cache.join(" · ")));
        }
        lines
    }
}

#[cfg(test)]
#[path = "tests/provider_native.rs"]
mod tests;
