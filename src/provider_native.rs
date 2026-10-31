// SPDX-License-Identifier: MIT
//! Native model inventories and Prometheus telemetry. No inference requests.
use crate::domain::{LlmTelemetry, RequestUsage, TelemetrySource};
use crate::formatting::bytes;
use crate::omlx::is_loopback_host;
use crate::providers::{counter, identifier};
use crate::runtime_diagnostics::ProbeIssue;
use crate::transport::MAX_HTTP_RESPONSE_BYTES;
use serde_json::Value;
use std::env;
use std::time::{Duration, Instant, SystemTime};

use std::collections::{BTreeMap, VecDeque};

#[derive(Default, Clone)]
pub(super) struct Endpoint {
    pub url: Option<String>,
    pub api_key: Option<String>,
    pub allow_remote_auth: bool,
}

impl Endpoint {
    pub fn from_env() -> Self {
        Self {
            url: env::var("MLXTOP_PROVIDER_URL").ok(),
            api_key: env::var("MLXTOP_PROVIDER_API_KEY").ok(),
            allow_remote_auth: env::var("MLXTOP_ALLOW_REMOTE_AUTH").is_ok_and(|value| value == "1"),
        }
    }

    pub fn is_remote(&self) -> bool {
        self.url
            .as_deref()
            .and_then(|url| url.parse::<ureq::http::Uri>().ok())
            .and_then(|uri| uri.host().map(|host| !is_loopback_host(host)))
            .unwrap_or(false)
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
        let base = self.base_url(port)?;
        if self.api_key.is_some() && self.is_remote() && !self.allow_remote_auth {
            return Err(ProbeIssue::RemoteAuthBlocked);
        }
        // No redirects or proxies: credentials stay at the configured endpoint.
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(2)))
            .timeout_connect(Some(Duration::from_millis(500)))
            .max_redirects(0)
            .proxy(None)
            .build()
            .into();
        let mut request = agent.get(format!("{base}{path}"));
        if let Some(key) = &self.api_key {
            if key.is_empty() || key.chars().any(char::is_control) {
                return Err(ProbeIssue::InvalidCredential);
            }
            request = request.header("Authorization", format!("Bearer {key}"));
        }
        let mut response = request.call().map_err(request_issue)?;
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
        if let Some(context) = counter(model, &["context_length"]) {
            details.push_str(&format!(" · context capacity {context} tokens"));
        }
    }
    result.details = Some(details);
    Some(result)
}

pub(super) fn lm_studio(value: &Value) -> Option<LlmTelemetry> {
    let models = value.get("models")?.as_array()?;
    let mut names = Vec::new();
    let mut contexts = Vec::new();
    for model in models {
        let instances = model.get("loaded_instances")?.as_array()?;
        if model.get("type").and_then(Value::as_str) == Some("embedding") {
            continue;
        }
        for instance in instances {
            names.push(identifier(instance, "id").or_else(|| identifier(model, "key"))?);
            contexts.push(counter(instance, &["config", "context_length"]));
        }
    }
    let mut result = inventory("LM Studio", names, "loaded");
    if let [Some(context)] = contexts.as_slice() {
        result
            .details
            .as_mut()?
            .push_str(&format!(" · context capacity {context} tokens"));
    }
    // size_bytes is the model file size, not live allocated memory.
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
        let current = self.values.get(name)?;
        let old = previous.values.get(name)?;
        if current.len() != old.len() {
            return None;
        }
        current.iter().try_fold(0.0, |sum, (key, value)| {
            let old = old.get(key)?;
            let delta = value - old;
            (delta.is_finite() && delta >= 0.0).then_some(sum + delta)
        })
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
pub(super) struct GenerationWindow {
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
    pub(super) fn observe(&mut self, now: Instant, live: f64) -> Option<(f64, bool)> {
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
}

fn series(provider: &str, metrics: &Metrics) -> Option<Series> {
    // mlx-serve publishes vLLM-named queues and its own series beside them. Sushi
    // serves the same gauges under `sushi:` instead of `mlx_serve:`.
    let (prefix, active, waiting, kv, native) = match provider {
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
    };
    if let Some(namespace) = native {
        // Tokens generated so far by running requests, tokens prefilled so far by the
        // request in flight, and the tokens restored from the prefix cache (over all
        // prompt tokens). vLLM's counters only move at request completion.
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

impl MetricsHistory {
    fn start_time(&self) -> Option<f64> {
        let (_, _, metrics) = self.previous.as_ref()?;
        metrics.sum("process_start_time_seconds")
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
        if let Some((old_provider, old_time, previous)) = &self.previous {
            let elapsed = now.saturating_duration_since(*old_time).as_secs_f64();
            let same_start = metrics.values.get("process_start_time_seconds")
                == previous.values.get("process_start_time_seconds");
            if !same_start {
                // A restarted server's gauges start over, so no earlier speed applies to them.
                self.generation.reset();
                self.sessions = SessionSpeeds::default();
                self.prefill = PrefillWindow::default();
            }
            if old_provider == provider && elapsed > 0.0 && elapsed <= 5.0 && same_start {
                result.generation_tps = metrics
                    .delta(previous, &series.generation)
                    .map(|n| n / elapsed);
                result.prefill_tps = metrics.delta(previous, &series.prompt).map(|n| n / elapsed);
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
        result.details = Some(details.join(" · "));
        self.previous = Some((provider.to_owned(), now, metrics));
        Some(result)
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

#[cfg(test)]
#[path = "tests/provider_native.rs"]
mod tests;
