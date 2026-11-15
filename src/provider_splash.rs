// SPDX-License-Identifier: MIT
//! Splash: one `/status` JSON carries the running requests, the engine's token counters, Metal
//! memory, its memory plan and the Mac's thermal state. No inference requests.
use crate::domain::{LlmTelemetry, RequestUsage, TelemetrySource};
use crate::formatting::{bytes, duration_label};
use crate::json::json_value;
use crate::model_facts::ModelFacts;
use crate::providers::{counter, identifier};
use serde_json::Value;
use std::time::{Duration, Instant, SystemTime};

/// How long the speed of a prefill that just finished stays shown once nothing is prefilling.
const PREFILL_HOLD: Duration = Duration::from_secs(3);
/// Counters further apart than this are not a current rate.
const MAX_RATE_SPAN: Duration = Duration::from_secs(30);
/// Fewer verify passes than this between two captures say too little about the draft: the
/// engine counts a pass and its tokens a moment apart, so one pass on the other side of a
/// capture moves the average of eight by an eighth.
const MIN_PASSES: u64 = 8;

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
    /// Draft tokens proposed and accepted (speculative decoding), when the
    /// model has a draft.
    drafted: Option<u64>,
    accepted: Option<u64>,
    /// Verify passes: a decode batch verifies a draft block for each request in it, so a batch
    /// of width w is w passes. Each pass emits the draft tokens it accepted and one more.
    passes: Option<u64>,
    /// What held decoding and first tokens up, where the engine counts it.
    holdups: Holdups,
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
            drafted: counter(status, &["metrics", "drafted_tokens"]),
            accepted: counter(status, &["metrics", "accepted_draft_tokens"]),
            passes: passes(status),
            holdups: Holdups::read(status),
        })
    }

    /// Tokens a verify pass emitted between two captures: the draft tokens it accepted and
    /// the one it adds. Unknown without a draft, and over too few passes to say.
    fn tokens_per_pass(&self, old: &Self) -> Option<f64> {
        let passes = self.passes?.checked_sub(old.passes?)?;
        let drafted = self.drafted?.checked_sub(old.drafted?)?;
        let accepted = self.accepted?.checked_sub(old.accepted?)?;
        (passes >= MIN_PASSES && accepted <= drafted).then(|| 1.0 + accepted as f64 / passes as f64)
    }

    /// The share of draft tokens accepted between two captures: what speculative decoding is
    /// earning now, not over the engine's life.
    fn acceptance(&self, old: &Self) -> Option<f64> {
        let drafted = self.drafted?.checked_sub(old.drafted?)?;
        let accepted = self.accepted?.checked_sub(old.accepted?)?;
        (drafted > 0 && accepted <= drafted).then(|| accepted as f64 / drafted as f64)
    }
}

/// Verify passes since the engine started, from its decode batches by width (`b1`, `b2`, …).
/// A width it does not name leaves them unknown rather than undercounted.
fn passes(status: &Value) -> Option<u64> {
    let widths = json_value(status, &["scheduler", "decode_batches_by_width"])?.as_object()?;
    widths
        .iter()
        .map(|(width, batches)| {
            let width: u64 = width.strip_prefix('b')?.parse().ok()?;
            width.checked_mul(batches.as_u64()?)
        })
        .try_fold(None, |total: Option<u64>, passes| {
            Some(Some(total.unwrap_or(0).checked_add(passes?)?))
        })
        .flatten()
}

/// What held work up, as Splash counts it since its engine started (1.6): decode gaps past
/// the scheduler's target by their first cause, each request's wait for its first token by
/// what held it, kernel pipelines Metal created while serving, and KV kept on the SSD.
#[derive(Clone, Default)]
struct Holdups {
    /// Each cause's gaps and their milliseconds.
    gaps: Vec<(String, u64, f64)>,
    /// Each part's milliseconds over all first tokens, and how many first tokens.
    first_token: Vec<(String, f64)>,
    first_tokens: Option<u64>,
    pipelines: Option<u64>,
    /// Releases of a spilling request's pages for a memory claim, and the KV history streams'
    /// bytes read with the milliseconds the reads took.
    spill_releases: Option<u64>,
    spill_read: Option<(u64, f64)>,
}

impl Holdups {
    fn read(status: &Value) -> Self {
        let entries = |path: &[&str]| {
            json_value(status, path)
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
        };
        let millis = |value: Option<&Value>| {
            value
                .and_then(Value::as_f64)
                .filter(|ms| ms.is_finite() && *ms >= 0.0)
        };
        let parts = entries(&["metrics", "ttft_parts_ms"]);
        let read_ms = json_value(status, &["kv_spill", "history_stream", "read_ms"])
            .and_then(|ms| millis(Some(ms)).or_else(|| millis(ms.get("total"))));
        Self {
            gaps: entries(&["metrics", "decode_gap_causes"])
                .into_iter()
                .filter_map(|(cause, counts)| {
                    Some((
                        cause.clone(),
                        counts.get("gaps")?.as_u64()?,
                        millis(counts.get("ms"))?,
                    ))
                })
                .collect(),
            first_token: parts
                .iter()
                .filter_map(|(part, shape)| Some(((*part).clone(), millis(shape.get("total"))?)))
                .collect(),
            first_tokens: parts
                .iter()
                .filter_map(|(_, shape)| shape.get("samples")?.as_u64())
                .max(),
            pipelines: counter(status, &["metal", "pipelines_created_while_serving"]),
            spill_releases: counter(status, &["kv_spill", "releases"]),
            spill_read: counter(status, &["kv_spill", "history_stream", "bytes"]).zip(read_ms),
        }
    }

    /// What held work up between two captures, in words: the problems, then the rest.
    fn since(&self, old: &Self) -> Said {
        let grown = |now: Option<u64>, then: Option<u64>| {
            now?.checked_sub(then?).filter(|grown| *grown > 0)
        };
        let mut said = Said::default();
        if let Some(n) = grown(self.pipelines, old.pipelines) {
            let noun = if n == 1 { "pipeline" } else { "pipelines" };
            said.problems
                .push(format!("{n} Metal {noun} compiled while serving"));
        }
        let mut gaps: Vec<(&str, u64, f64)> = self
            .gaps
            .iter()
            .filter_map(|(cause, gaps, ms)| {
                let (_, old_gaps, old_ms) = old.gaps.iter().find(|(old, ..)| old == cause)?;
                Some((
                    cause_name(cause)?,
                    grown(Some(*gaps), Some(*old_gaps))?,
                    (ms - old_ms).max(0.0),
                ))
            })
            .collect();
        gaps.sort_by(|a, b| b.2.total_cmp(&a.2));
        if !gaps.is_empty() {
            let gaps: Vec<String> = gaps
                .iter()
                .map(|(cause, n, ms)| format!("{n}× for {cause} ({})", millis_label(*ms)))
                .collect();
            said.notes
                .push(format!("decode paused {}", gaps.join(", ")));
        }
        if let Some(count) = grown(self.first_tokens, old.first_tokens) {
            let mut parts: Vec<(&str, f64)> = self
                .first_token
                .iter()
                .filter_map(|(part, total)| {
                    let (_, old_total) = old.first_token.iter().find(|(old, _)| old == part)?;
                    Some((
                        part_name(part)?,
                        (total - old_total).max(0.0) / count as f64,
                    ))
                })
                .collect();
            let whole: f64 = parts.iter().map(|(_, ms)| ms).sum();
            // A part under a twentieth of the wait explains none of it.
            parts.retain(|(_, ms)| *ms >= whole / 20.0 && *ms >= 1.0);
            parts.sort_by(|a, b| b.1.total_cmp(&a.1));
            if !parts.is_empty() {
                let head = if count == 1 {
                    format!("first token {}", millis_label(whole))
                } else {
                    format!("{count} first tokens, {} each", millis_label(whole))
                };
                let parts: Vec<String> = parts
                    .iter()
                    .map(|(part, ms)| format!("{} {part}", millis_label(*ms)))
                    .collect();
                said.notes.push(format!("{head}: {}", parts.join(", ")));
            }
        }
        if let Some(n) = grown(self.spill_releases, old.spill_releases) {
            said.notes
                .push(format!("KV released to the SSD {n}× for memory"));
        }
        if let Some(((now, now_ms), (then, then_ms))) = self.spill_read.zip(old.spill_read) {
            let read = now.checked_sub(then).filter(|read| *read > 0);
            if let Some(read) = read.filter(|_| now_ms > then_ms) {
                let rate = read as f64 * 1000.0 / (now_ms - then_ms);
                said.notes
                    .push(format!("KV read from the SSD at {}/s", bytes(rate as u64)));
            }
        }
        said
    }
}

/// What a decode gap's first cause is called, for the causes Splash documents.
fn cause_name(cause: &str) -> Option<&'static str> {
    Some(match cause {
        "prefill_unit" => "prefill",
        "claim_wait" => "memory",
        "ssd_read" => "SSD reads",
        "pool_release" => "pool releases",
        "replan" => "replans",
        "other" => "other work",
        _ => return None,
    })
}

/// What held a first token, for the parts Splash documents.
fn part_name(part: &str) -> Option<&'static str> {
    Some(match part {
        "own_prefill" => "own prefill",
        "claim_wait" => "waiting for memory",
        "decode" => "behind decoding",
        "other_prefill" => "behind another prefill",
        "rest" => "host and queue",
        _ => return None,
    })
}

fn millis_label(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.1} s", ms / 1000.0)
    } else {
        format!("{ms:.0} ms")
    }
}

/// Words about the engine: what is wrong, which leads, and what explains its speed.
#[derive(Clone, Default)]
struct Said {
    problems: Vec<String>,
    notes: Vec<String>,
}

/// The room Splash's memory plan has left for KV cache: the pages it planned for KV, less
/// those running requests hold, in bytes and in tokens. Pages that only hold cached prefixes
/// count as room, since cached blocks give way to running requests; the memory governor's
/// headroom would not, as it counts the caches grown into its budget as spent. Windowed and
/// spilling targets count their pages otherwise, so their room is unknown.
fn kv_room(status: &Value) -> Option<(u64, u64)> {
    let budget = json_value(status, &["memory_plan", "budget"])?;
    let present = |field: &str| budget.get(field).is_some_and(|value| !value.is_null());
    if present("kv_window")
        || present("kv_spill")
        || json_value(status, &["memory_plan", "valid"]).and_then(Value::as_bool) == Some(false)
    {
        return None;
    }
    let free = counter(budget, &["kv_capacity_pages"])?
        .saturating_sub(counter(status, &["kv", "pages_active"])?);
    let page_bytes = counter(budget, &["kv_page_bytes"]).filter(|bytes| *bytes > 0)?;
    let page_tokens = counter(budget, &["kv_page_tokens"]).filter(|tokens| *tokens > 0)?;
    free.checked_mul(page_bytes)
        .zip(free.checked_mul(page_tokens))
}

/// The served model as Splash's memory plan states it: the layers that keep KV, their format
/// and cost per token, the state each request holds whatever its length, the contexts and the
/// draft. Splash names neither the weights a pass reads nor the parameters.
fn splash_facts(status: &Value) -> ModelFacts {
    let model = |path: &[&str]| {
        json_value(status, &["memory_plan", "model"]).and_then(|model| json_value(model, path))
    };
    let number = |path: &[&str]| model(path).and_then(Value::as_u64);
    let small = |path: &[&str]| {
        number(path)
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0)
    };
    let page = number(&["kv_page_bytes"])
        .filter(|bytes| *bytes > 0)
        .zip(number(&["kv_page_tokens"]).filter(|tokens| *tokens > 0));
    ModelFacts {
        kv_layers: small(&["attention_layers"]),
        kv_heads: small(&["kv_heads"]),
        head_dim: small(&["head_dimension"]),
        kv_dtype: model(&["kv_format"])
            .and_then(Value::as_str)
            .map(sanitized)
            .filter(|format| !format.is_empty()),
        kv_bytes_per_token_reported: page
            .map(|(bytes, tokens)| (bytes as f64 / tokens as f64).round() as u64),
        // One request's state cell, which Splash stages whole.
        fixed_state_bytes: number(&["memory", "state_staging_bytes"]),
        context_configured: counter(status, &["capabilities", "max_context_tokens"])
            .or_else(|| counter(status, &["maximum_context_tokens"]))
            .filter(|tokens| *tokens > 0),
        context_trained: number(&["maximum_context_tokens"]).filter(|tokens| *tokens > 0),
        draft: number(&["memory", "draft_weights_bytes"]).map(|bytes| bytes > 0),
        ..ModelFacts::default()
    }
}

#[derive(Default)]
pub(super) struct SplashHistory {
    previous: Option<Counters>,
    generation: Option<f64>,
    tokens_per_pass: Option<f64>,
    acceptance: Option<f64>,
    /// What held work up over the last interval.
    holdups: Said,
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
}

impl SplashHistory {
    /// Rates from the counters' growth since the last capture. A busy engine loop answers with its
    /// last snapshot, so each capture is dated by its own age, and a capture already seen keeps the
    /// rates it gave.
    fn rates(&mut self, counters: Option<Counters>, prefilling: bool, now: Instant) -> Rates {
        self.prefilling_since = prefilling.then(|| self.prefilling_since.unwrap_or(now));
        let Some(current) = counters else {
            self.previous = None;
            self.forget();
            return Rates::default();
        };
        match &self.previous {
            Some(old) if current.at <= old.at && current.engine == old.engine => {}
            Some(old)
                if current.engine == old.engine
                    && current.decoded >= old.decoded
                    && current.prefilled >= old.prefilled
                    && current.prefill_ms >= old.prefill_ms
                    && current.at.duration_since(old.at) <= super::rate_span(MAX_RATE_SPAN) =>
            {
                let seconds = current.at.duration_since(old.at).as_secs_f64();
                self.generation = Some((current.decoded - old.decoded) as f64 / seconds);
                self.tokens_per_pass = current.tokens_per_pass(old);
                self.acceptance = current.acceptance(old);
                self.holdups = current.holdups.since(&old.holdups);
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
                self.forget();
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
            tokens_per_pass: self.tokens_per_pass,
            acceptance: self.acceptance,
            holdups: self.holdups.clone(),
        }
    }

    /// No interval to measure over: what the last one gave goes.
    fn forget(&mut self) {
        self.generation = None;
        self.prefill = None;
        self.tokens_per_pass = None;
        self.acceptance = None;
        self.holdups = Said::default();
    }

    /// Each running request's own output speed while it decodes alone, and a completed record for
    /// each one that has left the engine. With one request decoding, the engine's decode counters
    /// are that request's: its speed is decode tokens over decode time, and its final output is
    /// exact even when it finished between captures. Concurrent requests share the counters, so
    /// they get no per-request speed, and a finished one keeps its last observed output.
    fn track(
        &mut self,
        current: Vec<(RequestUsage, bool)>,
        counters: Option<&Counters>,
        engine: Option<(Option<String>, Option<u64>)>,
        observed: SystemTime,
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
                .position(|tracked| tracked.usage.id == usage.id)
                .map(|index| previous.swap_remove(index));
            let mut tracked = seen.unwrap_or(Tracked {
                usage: usage.clone(),
                decoded_at: None,
                alone_from: None,
                alone: true,
            });
            tracked.alone &= alone;
            if tracked.alone && decoding && tracked.alone_from.is_none() {
                tracked.alone_from =
                    counters.and_then(|counters| Some((counters.decoded, counters.decode_ms?)));
            }
            if tracked.alone {
                usage.output_tps = speed(tracked.alone_from);
            }
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
                } else if !tracked.alone {
                    usage.output_tps = None;
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
            .and_then(|ms| Duration::try_from_secs_f64(ms / 1000.0).ok())
            .unwrap_or(Duration::ZERO);
        let captured = now.checked_sub(age).unwrap_or(now);
        let counters = Counters::read(status, captured);
        let rates = self.rates(counters.clone(), prefilling > 0, now);
        let (kv_free_bytes, kv_free_tokens) = kv_room(status).unzip();

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
            draft_acceptance: rates.acceptance,
            tokens_per_pass: rates.tokens_per_pass,
            kv_free_bytes,
            kv_free_tokens,
            // Requests suspended for want of memory, to resume later by computing their
            // tokens again.
            preemptions_total: counter(status, &["cache", "resource_suspensions"]),
            model_facts: splash_facts(status),
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

        let mut details = Vec::new();
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
                let age = counter(r, &["age_ms"]).map(Duration::from_millis);
                let decoding = phase(r) != "prefill";
                let usage = RequestUsage {
                    provider: "Splash".into(),
                    model: model.clone().unwrap_or_default(),
                    id: identifier(r, "id")?,
                    prompt,
                    cached: None,
                    output: counter(r, &["generated_tokens"]),
                    output_tps: None,
                    completed: false,
                    queued: false,
                    prefilled: (!decoding)
                        .then(|| counter(r, &["prompt_processed"]))
                        .flatten()
                        .map(|done| done.min(prompt)),
                    idle_for: None,
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
        let (running_requests, finished) = self.track(current, counters.as_ref(), engine, observed);
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
        // Running requests whose memory claims wait to grow: the oldest's wait, and what they
        // lack.
        if let Some(n) = count(&["memory_governor", "claims", "waiting"]) {
            let wait = count(&["memory_governor", "claims", "oldest_ms"])
                .map(|ms| format!(" {}", duration_label(ms as f64 / 1000.0)))
                .unwrap_or_default();
            let lack = count(&["memory_governor", "claims", "bytes"])
                .map(|lack| format!(" {}", bytes(lack)))
                .unwrap_or_default();
            problems.push(format!("{n} waiting{wait} for{lack} more memory"));
        }
        problems.extend(rates.holdups.problems);
        if let Some(gate) = text(&["disk", "writes_gated"]).filter(|gate| *gate != "no") {
            problems.push(format!("SSD cache writes gated ({})", sanitized(gate)));
        }
        // What held this interval's decoding and first tokens up, before the percentiles.
        details.extend(rates.holdups.notes);
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

/// What the counters said over the last interval.
#[derive(Default)]
struct Rates {
    generation: Option<f64>,
    /// The speed and whether a prefill is under way or the reading is otherwise current.
    prefill: Option<(f64, bool)>,
    tokens_per_pass: Option<f64>,
    acceptance: Option<f64>,
    holdups: Said,
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
