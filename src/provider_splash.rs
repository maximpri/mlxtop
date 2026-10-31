// SPDX-License-Identifier: MIT
//! Splash: one `/status` JSON carries the running requests, the engine's token counters, Metal
//! memory and the Mac's thermal state. No inference requests.
use crate::domain::{LlmTelemetry, RequestUsage, TelemetrySource};
use crate::formatting::bytes;
use crate::json::json_value;
use crate::providers::{counter, identifier};
use serde_json::Value;
use std::time::{Duration, Instant, SystemTime};

/// How long the speed of a prefill that just finished stays shown once nothing is prefilling.
const PREFILL_HOLD: Duration = Duration::from_secs(3);
/// Counters further apart than this are not a current rate.
const MAX_RATE_SPAN: Duration = Duration::from_secs(30);

/// The engine's cumulative counters at the moment its status was captured.
#[derive(Clone)]
struct Counters {
    at: Instant,
    /// The HTTP server's instance and its engine restarts: counters reset with either.
    engine: (Option<String>, Option<u64>),
    decoded: u64,
    /// Decode wall time; with one request decoding, its own decode time.
    decode_ms: Option<f64>,
    prefilled: u64,
    prefill_ms: f64,
}

impl Counters {
    fn read(status: &Value, at: Instant) -> Option<Self> {
        Some(Self {
            at,
            engine: (
                json_value(status, &["instance"]).and_then(|i| identifier(i, "id")),
                counter(status, &["transport", "restarts"]),
            ),
            decoded: counter(status, &["metrics", "decode_output_tokens"])?,
            decode_ms: json_value(status, &["metrics", "decode_wall_ms"])
                .and_then(Value::as_f64)
                .filter(|ms| ms.is_finite() && *ms >= 0.0),
            prefilled: counter(status, &["metrics", "prefill_input_tokens"])?,
            prefill_ms: json_value(status, &["metrics", "prefill_wall_ms"])?
                .as_f64()
                .filter(|ms| ms.is_finite() && *ms >= 0.0)?,
        })
    }
}

#[derive(Default)]
pub(super) struct SplashHistory {
    previous: Option<Counters>,
    generation: Option<f64>,
    /// The latest prefill speed and when it was measured.
    prefill: Option<(f64, Instant)>,
    /// When the prefills under way began to be seen.
    prefilling_since: Option<Instant>,
    /// Running requests last seen, to report each one's speed and completion.
    tracked: Vec<Tracked>,
    /// The engine the tracked requests ran on: request IDs restart with it.
    tracked_engine: Option<(Option<String>, Option<u64>)>,
}

/// A running request between captures.
struct Tracked {
    usage: RequestUsage,
    /// Engine decode counters when it was last seen.
    decoded_at: Option<u64>,
    /// Engine decode tokens and time when it was first seen decoding alone.
    alone_from: Option<(u64, f64)>,
    /// No other request ran at any capture since it started decoding alone.
    alone: bool,
    window: super::native::GenerationWindow,
}

impl SplashHistory {
    /// Rates from the counters' growth since the last capture. A busy engine loop answers with its
    /// last snapshot, so each capture is dated by its own age, and a capture already seen keeps the
    /// rates it gave.
    fn rates(&mut self, counters: Option<Counters>, prefilling: bool, now: Instant) -> Rates {
        self.prefilling_since = prefilling.then(|| self.prefilling_since.unwrap_or(now));
        let Some(current) = counters else {
            self.previous = None;
            self.generation = None;
            self.prefill = None;
            return Rates::default();
        };
        match &self.previous {
            Some(old) if current.at <= old.at && current.engine == old.engine => {}
            Some(old)
                if current.engine == old.engine
                    && current.decoded >= old.decoded
                    && current.prefilled >= old.prefilled
                    && current.prefill_ms >= old.prefill_ms
                    && current.at.duration_since(old.at) <= MAX_RATE_SPAN =>
            {
                let seconds = current.at.duration_since(old.at).as_secs_f64();
                self.generation = Some((current.decoded - old.decoded) as f64 / seconds);
                // Prefill tokens over the prefill commands' own GPU time: the counters move once per
                // chunk, so dividing by the poll interval would read 0 or a multiple of the speed.
                let tokens = (current.prefilled - old.prefilled) as f64;
                let millis = current.prefill_ms - old.prefill_ms;
                if tokens > 0.0 && millis > 0.0 {
                    self.prefill = Some((tokens * 1000.0 / millis, now));
                }
                self.previous = Some(current);
            }
            _ => {
                self.generation = None;
                self.prefill = None;
                self.previous = Some(current);
            }
        }
        let Some(generation) = self.generation else {
            return Rates::default();
        };
        let held = |at: Instant| now.saturating_duration_since(at) <= PREFILL_HOLD;
        let prefill = match (self.prefill, self.prefilling_since) {
            // Between chunks of the prompts in prefill, their speed is the last chunk's; nothing
            // until their first chunk lands.
            (Some((rate, at)), Some(since)) if at >= since => Some((rate, true)),
            (_, Some(_)) => None,
            (Some((rate, at)), None) if held(at) => Some((rate, false)),
            _ => Some((0.0, true)),
        };
        Rates {
            generation: Some(generation),
            prefill,
        }
    }

    /// Each request's own output speed, and a completed record for each one that left the engine.
    /// Concurrent rates use each request's own token counts over the capture interval. With one
    /// request decoding alone, engine counters give its decode time and exact final output even
    /// when it finishes between captures. Concurrent completions keep their last observed output;
    /// a recorded completion can supply final counts and request timings.
    fn track(
        &mut self,
        current: Vec<(RequestUsage, bool)>,
        counters: Option<&Counters>,
        engine: Option<(Option<String>, Option<u64>)>,
        observed: SystemTime,
        captured: Instant,
    ) -> (Vec<RequestUsage>, Vec<RequestUsage>) {
        if engine != self.tracked_engine {
            // A restarted engine reuses request IDs and forgets its requests.
            self.tracked.clear();
            self.tracked_engine = engine;
        }
        let alone = current.len() == 1;
        let speed = |from: Option<(u64, f64)>| {
            let (tokens, millis) = from?;
            let counters = counters?;
            let elapsed = counters.decode_ms? - millis;
            (elapsed > 0.0 && counters.decoded >= tokens)
                .then(|| (counters.decoded - tokens) as f64 * 1000.0 / elapsed)
        };
        let mut previous = std::mem::take(&mut self.tracked);
        let mut running = Vec::with_capacity(current.len());
        for (mut usage, decoding) in current {
            let seen = previous
                .iter()
                .position(|tracked| {
                    tracked.usage.id == usage.id && tracked.usage.prompt == usage.prompt
                })
                .map(|index| previous.swap_remove(index));
            let mut tracked = seen.unwrap_or(Tracked {
                usage: usage.clone(),
                decoded_at: None,
                alone_from: None,
                alone: true,
                window: super::native::GenerationWindow::default(),
            });
            tracked.alone &= alone;
            if tracked.alone && decoding && tracked.alone_from.is_none() {
                tracked.alone_from =
                    counters.and_then(|counters| Some((counters.decoded, counters.decode_ms?)));
            }
            let request_speed = decoding
                .then(|| {
                    usage
                        .output
                        .and_then(|output| tracked.window.observe(captured, output as f64))
                        .map(|(rate, _)| rate)
                })
                .flatten();
            usage.output_tps = usage.output_tps.or_else(|| {
                if tracked.alone {
                    speed(tracked.alone_from).or(request_speed)
                } else {
                    request_speed
                }
            });
            tracked.decoded_at = counters.map(|counters| counters.decoded);
            tracked.usage = usage.clone();
            self.tracked.push(tracked);
            running.push(usage);
        }
        // Whatever is no longer running has finished. Only with nothing else running do the
        // counters since its last capture belong to it alone.
        let quiet = running.is_empty();
        let finished = previous
            .into_iter()
            .map(|tracked| {
                let mut usage = tracked.usage;
                if tracked.alone && quiet {
                    if let Some((now, then)) = counters
                        .map(|counters| counters.decoded)
                        .zip(tracked.decoded_at)
                    {
                        usage.output = Some(usage.output.unwrap_or(0) + now.saturating_sub(then));
                    }
                    usage.output_tps = speed(tracked.alone_from).or(usage.output_tps);
                }
                usage.completed = true;
                usage.observed_at = Some(observed);
                usage
            })
            .collect();
        (running, finished)
    }

    pub fn observe(&mut self, status: &Value, now: Instant) -> Option<LlmTelemetry> {
        // Even an engine that has not started answers with `ready`.
        let ready = status.get("ready")?.as_bool()?;
        let model = json_value(status, &["instance"]).and_then(|i| identifier(i, "model"));
        let observed = SystemTime::now();
        let requests = status.get("active_requests").and_then(Value::as_array);
        let count = |phases: &[&str]| {
            requests.map(|rs| rs.iter().filter(|r| phases.contains(&phase(r))).count() as u64)
        };
        let running = ["prefill", "decode", "waiting_mask"];
        let active = count(&running);
        let waiting = count(&["queued", "waiting_resources", "waiting_prefix"]);
        let prefilling = count(&["prefill"]).unwrap_or(0);
        let generating = count(&["decode", "waiting_mask"]).unwrap_or(0);

        let transport = |field: &str| {
            json_value(status, &["transport", field]).and_then(Value::as_bool) == Some(true)
        };
        let weights = |field: &str| {
            json_value(status, &["weights", field]).and_then(Value::as_bool) == Some(true)
        };
        let text = |path: &[&str]| json_value(status, path).and_then(Value::as_str);
        let released = weights("released") && !weights("restoring");
        let state = if transport("stopped") {
            "error"
        } else if transport("recovering") {
            "restarting"
        } else if !ready && text(&["memory_pressure"]) == Some("critical") {
            "paused"
        } else if !ready || weights("restoring") {
            "loading"
        } else if prefilling > 0 {
            "prefilling"
        } else if generating > 0 {
            "generating"
        } else if waiting.unwrap_or(0) > 0 {
            "waiting"
        } else {
            "idle"
        };

        // The snapshot of a busy loop is as old as `status_age_ms`.
        let age = json_value(status, &["transport", "status_age_ms"])
            .and_then(Value::as_f64)
            .filter(|ms| ms.is_finite() && *ms >= 0.0)
            .map_or(Duration::ZERO, |ms| Duration::from_secs_f64(ms / 1000.0));
        let captured = now.checked_sub(age).unwrap_or(now);
        let counters = Counters::read(status, captured);
        let rates = self.rates(counters.clone(), prefilling > 0, now);

        let mut result = LlmTelemetry {
            source: TelemetrySource::Live,
            observed_at: Some(observed),
            provider: Some("Splash".into()),
            status: Some(state.into()),
            model: Some(model.clone().unwrap_or_else(|| "none".into())),
            active_requests: active,
            waiting_requests: waiting,
            generation_tps: rates.generation,
            generation_tps_live: rates.generation.is_some(),
            prefill_tps: rates.prefill.map(|(rate, _)| rate),
            prefill_tps_live: rates.prefill.is_some_and(|(_, live)| live),
            model_memory: counter(status, &["memory_actual", "current_bytes"]),
            model_memory_max: counter(status, &["memory_governor", "limit_bytes"])
                .filter(|limit| *limit > 0),
            ..LlmTelemetry::default()
        };

        // Prompt tokens restored from the prefix cache, over all prompt tokens served.
        let reused = counter(status, &["cache", "reused_tokens"]);
        let prefilled = counter(status, &["metrics", "prefill_input_tokens"]);
        if let Some((reused, prefilled)) = reused.zip(prefilled) {
            let total = reused.saturating_add(prefilled);
            result.total_prompt_tokens = Some(total);
            result.total_cached_tokens = Some(reused);
            if total > 0 {
                result.prefix_hit_rate = Some(reused as f64 / total as f64 * 100.0);
                result.cache_efficiency = result.prefix_hit_rate;
            }
        }

        let mut details = crate::runtime_features::splash_details(status);
        let current: Vec<(RequestUsage, bool)> = requests
            .into_iter()
            .flatten()
            .filter(|r| running.contains(&phase(r)))
            .filter_map(|r| {
                let prompt = counter(r, &["prompt_tokens"])?;
                if phase(r) == "prefill" && prefilling == 1 {
                    if let Some(done) = counter(r, &["prompt_processed"]).filter(|_| prompt > 0) {
                        details.push(format!(
                            "prefill {done} / {prompt} tokens ({:.0}%)",
                            done as f64 / prompt as f64 * 100.0
                        ));
                    }
                }
                let age = r
                    .get("age_ms")
                    .and_then(Value::as_f64)
                    .and_then(|ms| Duration::try_from_secs_f64(ms / 1000.0).ok());
                let decoding = phase(r) != "prefill";
                let usage = RequestUsage {
                    provider: "Splash".into(),
                    model: model.clone().unwrap_or_default(),
                    id: identifier(r, "id")?,
                    prompt,
                    cached: counter(r, &["cached_tokens"]).filter(|n| *n <= prompt),
                    output: counter(r, &["generated_tokens"]),
                    output_tps: crate::json::request_rate(r, &["tokens_per_second"]),
                    completed: false,
                    queued: false,
                    prefilled: (!decoding)
                        .then(|| counter(r, &["prompt_processed"]))
                        .flatten()
                        .map(|done| done.min(prompt)),
                    ttft_ms: None,
                    observed_at: Some(
                        age.and_then(|age| observed.checked_sub(age))
                            .unwrap_or(observed),
                    ),
                };
                Some((usage, decoding))
            })
            .collect();
        let engine = counters.as_ref().map(|counters| counters.engine.clone());
        let (running_requests, finished) =
            self.track(current, counters.as_ref(), engine, observed, captured);
        if let [only] = running_requests.as_slice() {
            result.prompt_tokens = Some(only.prompt);
            result.output_tokens = only.output;
        }
        result.requests = running_requests.into_iter().chain(finished).collect();

        // Problems lead: the details line is cut to fit its panel.
        let mut problems = Vec::new();
        let count = |path: &[&str]| counter(status, path).filter(|n| *n > 0);
        let outcomes: Vec<String> = [("failed", "failed"), ("cancelled", "cancelled")]
            .into_iter()
            .filter_map(|(field, label)| Some(format!("{} {label}", count(&["requests", field])?)))
            .collect();
        if !outcomes.is_empty() {
            let submitted = count(&["requests", "submitted"])
                .map(|n| format!(" of {n} requests"))
                .unwrap_or_default();
            problems.push(format!("{}{submitted}", outcomes.join(" · ")));
        }
        if json_value(status, &["metal", "healthy"]).and_then(Value::as_bool) == Some(false) {
            problems.push("Metal unhealthy".into());
        }
        if let Some(n) = count(&["metrics", "metal_failures"]) {
            problems.push(format!("{n} Metal failures"));
        }
        if let Some(n) = count(&["metrics", "capacity_failures"]) {
            problems.push(format!("{n} capacity failures"));
        }
        if let Some(n) = count(&["admission", "waiting_memory"]) {
            problems.push(format!("{n} waiting for memory"));
        }
        if let Some(gate) = text(&["disk", "writes_gated"]).filter(|gate| *gate != "no") {
            problems.push(format!("SSD cache writes gated ({})", sanitized(gate)));
        }
        if let Some(context) = counter(status, &["maximum_context_tokens"]).filter(|n| *n > 0) {
            details.push(format!("context capacity {context} tokens"));
        }
        if let Some(rate) = json_value(status, &["metrics", "draft_acceptance_rate"])
            .and_then(Value::as_f64)
            .filter(|rate| (0.0..=1.0).contains(rate))
            .filter(|_| counter(status, &["metrics", "drafted_tokens"]).unwrap_or(0) > 0)
        {
            details.push(format!("DFlash draft acceptance {:.0}%", rate * 100.0));
        }
        let percentiles = |metric: &str| {
            let at = |p: &str| {
                json_value(status, &["metrics", metric, p])
                    .and_then(Value::as_f64)
                    .filter(|ms| ms.is_finite() && *ms > 0.0)
            };
            at("p50").map(|p50| match at("p95") {
                Some(p95) => format!("p50 {p50:.0} ms · p95 {p95:.0} ms"),
                None => format!("p50 {p50:.0} ms"),
            })
        };
        if let Some(ttft) = percentiles("ttft_ms") {
            details.push(format!("server TTFT {ttft}"));
        }
        if let Some(itl) = percentiles("itl_ms") {
            details.push(format!("inter-token {itl}"));
        }
        if let Some(peak) = count(&["memory_actual", "peak_bytes"]) {
            let headroom = counter(status, &["memory_governor", "headroom_bytes"])
                .map(|n| format!(" · headroom {}", bytes(n)))
                .unwrap_or_default();
            details.push(format!("Metal peak {}{headroom}", bytes(peak)));
        }
        if released {
            details.push("weights released while idle".into());
        }
        if let Some(thermal) = text(&["thermal_state"]).filter(|s| *s != "nominal") {
            problems.push(format!("thermal state {}", sanitized(thermal)));
        }
        if let Some(pressure) = text(&["memory_pressure"]).filter(|s| *s != "normal") {
            problems.push(format!("memory pressure {}", sanitized(pressure)));
        }
        problems.append(&mut details);
        result.details = (!problems.is_empty()).then(|| problems.join(" · "));
        Some(result)
    }
}

#[derive(Default)]
struct Rates {
    generation: Option<f64>,
    /// The speed and whether a prefill is under way or the reading is otherwise current.
    prefill: Option<(f64, bool)>,
}

fn phase(request: &Value) -> &str {
    request
        .get("phase")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn sanitized(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(20)
        .collect()
}

#[cfg(test)]
#[path = "tests/provider_splash.rs"]
mod tests;
