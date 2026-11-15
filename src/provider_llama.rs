// SPDX-License-Identifier: MIT
//! llama-server (llama.cpp): its slots, its Prometheus counters and what it
//! says of itself and its model, read without waking it.
//!
//! llama-server can unload its model after `--sleep-idle-seconds` of idle
//! and load it again for the next request. `/health`, `/props`, `/models`
//! and `/metrics` neither wake it nor count as activity. `/slots` does both:
//! its handler waits for the server to wake, even when the endpoint is off,
//! and every task but a metrics scrape restarts the idle timer (README@v1.0.0
//! "Sleeping on Idle"; server-context.cpp@v1.0.0 `get_slots`;
//! server-queue.cpp@v1.0.0 `task_resets_idle_timer`). So `/slots` is read
//! only while `/metrics` counts a request in flight, which is also the only
//! time it has something to say; without `/metrics`, only while `/props`
//! says the server is awake.
use super::{counter, identifier, rate_span};
use crate::domain::{LlmTelemetry, RequestUsage, Sample, TelemetrySource};
use crate::formatting::compact_tokens;
use crate::model_facts::{bits_per_weight, ModelFacts};
use serde_json::Value;
use std::time::{Duration, Instant, SystemTime};

/// How often `/props` and `/v1/models` are asked what the server serves
/// and whether it sleeps.
const DESCRIBE_EVERY: Duration = Duration::from_secs(10);

/// Without `/metrics`, how often a sleeping server is asked whether a
/// request woke it.
const ASLEEP_RECHECK: Duration = Duration::from_secs(5);

/// A `/slots` read restarts the server's idle timer, and llama-server sleeps
/// no sooner than a second after its last task: within this time of the last
/// read it is still awake, unless told to sleep after a single idle second.
const SLOTS_KEEP_AWAKE: Duration = Duration::from_secs(2);

/// Samples further apart than this measure an average, not the current speed.
const LLAMA_RATE_SPAN: Duration = Duration::from_secs(30);

/// What llama-server says of itself beyond the readings every runtime
/// shares, told in Diagnostics.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct LlamaReadings {
    /// The server unloaded its model after `--sleep-idle-seconds` of idle;
    /// the next request loads it again.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) asleep: bool,
    /// Slots: how many requests it serves at once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) slots: Option<u64>,
    /// Busy slots per decode call since the server started: how many
    /// requests share a batch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) busy_per_decode: Option<f64>,
    /// The prompts being read: tokens in their slots so far, and the part
    /// of them that came from the cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reading: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reading_cached: Option<u64>,
    /// Speculative decoding since the server started: the tokens each
    /// verify pass emitted, and the share of drafted tokens accepted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) draft_per_pass: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) draft_acceptance: Option<f64>,
}

/// Diagnostics' lines for llama-server: each reading once, only when known,
/// and only what the screen does not already show.
pub(crate) fn lines(sample: &Sample) -> Vec<String> {
    if sample.llm_provider != "llama.cpp" {
        return Vec::new();
    }
    let llama = &sample.llama;
    let mut lines = Vec::new();
    if llama.asleep {
        lines.push(
            "Asleep: the model is unloaded until the next request loads it again; mlxtop does not wake it."
                .into(),
        );
    }
    if let Some(left) = sample
        .llm_kv_free_tokens
        .filter(|_| sample.llm_active_requests.is_some_and(|active| active > 0))
    {
        lines.push(format!(
            "Context left: {} tokens, for the running request nearest its limit",
            compact_tokens(left)
        ));
    }
    if let Some(read) = llama.reading {
        lines.push(match llama.reading_cached.filter(|cached| *cached > 0) {
            Some(cached) => format!(
                "Reading a prompt: {} tokens so far, {} of them from the cache",
                compact_tokens(read),
                compact_tokens(cached)
            ),
            None => format!("Reading a prompt: {} tokens so far", compact_tokens(read)),
        });
    }
    // One slot is never more than one busy per step: nothing to say.
    if let Some(slots) = llama.slots.filter(|slots| *slots > 1) {
        lines.push(match llama.busy_per_decode {
            Some(busy) => {
                format!("Slots: {slots} · {busy:.1} busy per decode step since the server started")
            }
            None => format!("Slots: {slots}"),
        });
    }
    if let Some(per_pass) = llama.draft_per_pass {
        lines.push(match llama.draft_acceptance {
            Some(share) => format!(
                "Speculative decoding: {per_pass:.1} tokens per verify pass, {:.0}% of drafted tokens accepted, since the server started",
                share * 100.0
            ),
            None => format!(
                "Speculative decoding: {per_pass:.1} tokens per verify pass since the server started"
            ),
        });
    }
    lines
}

/// One running request: its slot, its task and its output tokens so far.
pub(super) type SlotProgress = (u64, u64, u64);

/// One running request's output growth since the last sample: its slot,
/// its task and the tokens it added.
type Growth = (u64, u64, u64);

/// A slot's output tokens. Newer llama-server builds report `next_token` as a
/// list with one entry per sequence; older ones as a single object.
pub(super) fn slot_decoded(slot: &Value) -> Option<u64> {
    match slot.get("next_token")? {
        Value::Array(entries) => entries.iter().try_fold(0_u64, |sum, entry| {
            sum.checked_add(counter(entry, &["n_decoded"])?)
        }),
        _ => counter(slot, &["next_token", "n_decoded"]),
    }
}

/// Output tokens of every running request by slot and task, from `/slots`.
/// `None` when the reply does not have that shape.
pub(super) fn llama_progress(slots: &Value) -> Option<Vec<SlotProgress>> {
    slots
        .as_array()?
        .iter()
        .filter(|slot| slot["is_processing"] == true)
        .map(|slot| {
            Some((
                counter(slot, &["id"])?,
                counter(slot, &["id_task"])?,
                slot_decoded(slot)?,
            ))
        })
        .collect()
}

/// One slot of `/slots`, as server-context.cpp@v1.0.0 `server_slot::to_json`
/// writes it with prompt text left out (the default). A field a build does
/// not send is `None`.
#[derive(Clone, Debug, Default, PartialEq)]
struct Slot {
    id: Option<u64>,
    /// The slot's context limit in tokens.
    n_ctx: Option<u64>,
    speculative: Option<bool>,
    processing: bool,
    /// Its task, current or last.
    task: Option<u64>,
    /// Tokens its context holds: the prompt read so far, then the answer.
    held: Option<u64>,
    /// Prompt tokens it computed, and those it took from the cache.
    computed: Option<u64>,
    cached: Option<u64>,
    /// Output tokens.
    decoded: Option<u64>,
}

impl Slot {
    /// Every slot of a `/slots` reply; `None` unless each says whether it
    /// is processing.
    fn read_all(slots: &Value) -> Option<Vec<Self>> {
        slots
            .as_array()?
            .iter()
            .map(|slot| {
                Some(Self {
                    id: counter(slot, &["id"]),
                    n_ctx: counter(slot, &["n_ctx"]).filter(|n| *n > 0),
                    speculative: slot.get("speculative").and_then(Value::as_bool),
                    processing: slot.get("is_processing")?.as_bool()?,
                    task: counter(slot, &["id_task"]),
                    held: counter(slot, &["n_prompt_tokens"]),
                    computed: counter(slot, &["n_prompt_tokens_processed"]),
                    cached: counter(slot, &["n_prompt_tokens_cache"]),
                    decoded: slot_decoded(slot),
                })
            })
            .collect()
    }

    /// Answering: its prompt is read and its first token is out.
    fn generating(&self) -> bool {
        self.processing && self.decoded.is_some_and(|decoded| decoded > 0)
    }

    /// Still reading its prompt.
    fn reading(&self) -> bool {
        self.processing && self.decoded == Some(0)
    }

    /// The whole prompt, once it is read: what came from the cache and what
    /// was computed. While it is read, `/slots` does not say how long it is.
    fn prompt(&self) -> Option<u64> {
        if !self.generating() {
            return None;
        }
        self.cached?.checked_add(self.computed?)
    }

    /// Tokens the slot can still take before its context limit.
    fn room(&self) -> Option<u64> {
        Some(self.n_ctx?.saturating_sub(self.held?))
    }
}

/// What `/v1/models` says of the model, as server-context.cpp@v1.0.0
/// `get_res_model_info` writes it. Its `meta` is `null` while the model
/// loads.
#[derive(Clone, Debug, Default, PartialEq)]
struct Catalogue {
    model: Option<String>,
    parameters: Option<u64>,
    /// Bytes of its weights.
    bytes: Option<u64>,
    /// A slot's context limit, and the context it was trained for.
    context: Option<u64>,
    trained: Option<u64>,
    ftype: Option<String>,
}

impl Catalogue {
    fn read(models: &Value) -> Self {
        // One model, unless the server is a router listing several.
        let model = models
            .get("data")
            .and_then(Value::as_array)
            .and_then(|data| match data.as_slice() {
                [only] => Some(only),
                _ => None,
            });
        let meta = model
            .and_then(|model| model.get("meta"))
            .filter(|meta| meta.is_object());
        let whole = |key: &str| {
            meta.and_then(|meta| counter(meta, &[key]))
                .filter(|n| *n > 0)
        };
        Self {
            model: model
                .and_then(|model| model.get("id"))
                .and_then(Value::as_str)
                .and_then(model_name),
            parameters: whole("n_params"),
            bytes: whole("size"),
            context: whole("n_ctx"),
            trained: whole("n_ctx_train"),
            ftype: meta.and_then(|meta| ftype(meta.get("ftype")?)),
        }
    }
}

/// What `/props` says of the server, as server-context.cpp@v1.0.0
/// `get_res_props` writes it (its chat template and paths are not read).
#[derive(Clone, Debug, Default, PartialEq)]
struct Props {
    slots: Option<u64>,
    /// A slot's context limit.
    context: Option<u64>,
    /// `b<build>-<commit>`.
    build: Option<String>,
    ftype: Option<String>,
    asleep: Option<bool>,
    /// Whether `/slots` is on: asked for when it is off, it still wakes the
    /// server.
    slots_on: Option<bool>,
}

impl Props {
    fn read(props: &Value) -> Self {
        Self {
            slots: counter(props, &["total_slots"]).filter(|n| *n > 0),
            context: counter(props, &["default_generation_settings", "n_ctx"]).filter(|n| *n > 0),
            build: identifier(props, "build_info")
                .filter(|build| !build.trim().is_empty())
                .map(|build| build.chars().take(60).collect()),
            ftype: props.get("model_ftype").and_then(ftype),
            asleep: props.get("is_sleeping").and_then(Value::as_bool),
            slots_on: props.get("endpoint_slots").and_then(Value::as_bool),
        }
    }
}

/// A model's name as llama-server gives it: its alias, or by default the
/// path it was loaded from, of which only the file name is kept.
fn model_name(id: &str) -> Option<String> {
    let name = id.rsplit(['/', '\\']).next().unwrap_or(id);
    let name: String = name.chars().filter(|c| !c.is_control()).take(120).collect();
    (!name.trim().is_empty()).then_some(name)
}

/// How the weights are stored, as llama.cpp names it (`Q4_K - Medium`); a
/// type it does not know is unknown.
fn ftype(value: &Value) -> Option<String> {
    let name: String = value
        .as_str()?
        .chars()
        .filter(|c| !c.is_control())
        .take(40)
        .collect();
    (!name.trim().is_empty() && !name.contains("unknown")).then_some(name)
}

/// llama-server's cumulative counters: `*_tokens_total` and the seconds
/// spent on them. Their growth between two scrapes is the average speed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LlamaTotals {
    predicted: f64,
    predicted_seconds: f64,
    prompt: f64,
    prompt_seconds: f64,
}

/// What `/metrics` adds beyond rates and queue counts, as
/// server-task.cpp@v1.0.0 `server_task_result_metrics::to_metrics` prints it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Counters {
    /// Prompt tokens computed, and those reused from the cache.
    prompt: Option<f64>,
    cached: Option<f64>,
    busy_per_decode: Option<f64>,
    /// Speculative decoding: draft tokens, those accepted, and verify passes.
    drafted: Option<f64>,
    accepted: Option<f64>,
    passes: Option<f64>,
}

impl Counters {
    fn read(text: &str) -> Self {
        Self {
            prompt: metric(text, "llamacpp:prompt_tokens_total"),
            cached: metric(text, "llamacpp:prompt_tokens_cached_total"),
            busy_per_decode: metric(text, "llamacpp:n_busy_slots_per_decode")
                .filter(|busy| *busy > 0.0),
            drafted: metric(text, "llamacpp:spec_decode_num_draft_tokens_total"),
            accepted: metric(text, "llamacpp:spec_decode_num_accepted_tokens_total"),
            passes: metric(text, "llamacpp:spec_decode_num_drafts_total"),
        }
    }
}

/// How far a value llama-server printed may be from the true one: it
/// prints six significant digits, so a whole number below a million is
/// exact and a larger one moves in steps (`1.23457e+06`).
fn printed_error(value: f64) -> f64 {
    if value < 1e6 && value.fract() == 0.0 {
        return 0.0;
    }
    0.5 * 10_f64.powi(value.abs().log10().floor() as i32 - 5)
}

/// A request's id: its task, which llama-server's log names too. Task
/// numbers start again when the server restarts, so the ids after a
/// restart seen while watching carry how many there were.
fn request_id(task: u64, restarts: u32) -> String {
    if restarts == 0 {
        task.to_string()
    } else {
        format!("{task}-{restarts}")
    }
}

/// llama-server between polls: the live rate from each running request's
/// output growth, the averages from its counters, what it says of itself,
/// and when it may be asked about its slots.
///
/// llama-server publishes generation speed only as an average over finished
/// requests. The live rate is the growth of each running request's output
/// between two samples; a request counts only when the same slot runs the same
/// task in both, so a finished or newly started request never reads as speed.
/// It must also have decoded tokens already in the earlier sample: a slot
/// still prefilling stays at zero output and is not stalled, and the first
/// sample after prefill spans prompt time as well.
#[derive(Default)]
pub(super) struct LlamaRate {
    previous: Option<(Vec<SlotProgress>, Instant)>,
    /// The server's cumulative token and time counters at the last scrape,
    /// and the averages last computed from them.
    totals: Option<LlamaTotals>,
    averages: (Option<f64>, Option<f64>),
    /// What `/v1/models` and `/props` last said, and when they were asked.
    catalogue: Option<Catalogue>,
    props: Option<Props>,
    described_at: Option<Instant>,
    /// When `/slots` last answered: a read restarts the idle timer.
    slots_read: Option<Instant>,
    /// Whether the slots draft speculatively, as `/slots` last said.
    speculative: Option<bool>,
    /// Restarts seen while watching, and the latest task of each slot.
    restarts: u32,
    tasks: Vec<(u64, u64)>,
    /// Accepted draft tokens and verify passes where the tokens-per-pass
    /// average being formed starts.
    drafts: Option<(f64, f64)>,
}

impl LlamaRate {
    /// The average generation and prefill speeds from the counters' growth,
    /// held while nothing grows. `None` when the server has no counters.
    pub(super) fn averages(&mut self, metrics: &str) -> Option<(Option<f64>, Option<f64>)> {
        let totals = LlamaTotals {
            predicted: metric(metrics, "llamacpp:tokens_predicted_total")?,
            predicted_seconds: metric(metrics, "llamacpp:tokens_predicted_seconds_total")?,
            prompt: metric(metrics, "llamacpp:prompt_tokens_total")?,
            prompt_seconds: metric(metrics, "llamacpp:prompt_seconds_total")?,
        };
        let rate =
            |tokens: f64, seconds: f64| (tokens > 0.0 && seconds > 0.0).then(|| tokens / seconds);
        if let Some(old) = self.totals {
            if totals.predicted < old.predicted || totals.prompt < old.prompt {
                // A restarted server counts from zero again.
                self.averages = (None, None);
            } else {
                if let Some(speed) = rate(
                    totals.predicted - old.predicted,
                    totals.predicted_seconds - old.predicted_seconds,
                ) {
                    self.averages.0 = Some(speed);
                }
                if let Some(speed) = rate(
                    totals.prompt - old.prompt,
                    totals.prompt_seconds - old.prompt_seconds,
                ) {
                    self.averages.1 = Some(speed);
                }
            }
        }
        self.totals = Some(totals);
        Some(self.averages)
    }

    /// Each running request's output growth since the last sample, by slot
    /// and task, over the seconds between: only requests seen decoding in
    /// both samples. `None` without an earlier sample close enough.
    fn growth(&self, current: &[SlotProgress], now: Instant) -> Option<(Vec<Growth>, f64)> {
        let (old, at) = self.previous.as_ref()?;
        let span = now.checked_duration_since(*at)?;
        if span.is_zero() || span > rate_span(LLAMA_RATE_SPAN) {
            return None;
        }
        let grown = current
            .iter()
            .filter_map(|(slot, task, decoded)| {
                let before = old
                    .iter()
                    .find(|(s, t, _)| s == slot && t == task)
                    .map(|(_, _, n)| *n)?;
                (before > 0 && before <= *decoded).then(|| (*slot, *task, decoded - before))
            })
            .collect();
        Some((grown, span.as_secs_f64()))
    }

    pub(super) fn observe(
        &mut self,
        current: Option<Vec<SlotProgress>>,
        now: Instant,
    ) -> Option<f64> {
        let Some(current) = current else {
            self.previous = None;
            return None;
        };
        let rate = self
            .growth(&current, now)
            .filter(|(grown, _)| !grown.is_empty())
            .map(|(grown, seconds)| {
                grown
                    .iter()
                    .fold(0_u64, |sum, (_, _, tokens)| sum.saturating_add(*tokens))
                    as f64
                    / seconds
            });
        self.previous = Some((current, now));
        rate
    }

    fn asleep(&self) -> Option<bool> {
        self.props.as_ref()?.asleep
    }

    fn set_awake(&mut self) {
        if let Some(props) = &mut self.props {
            props.asleep = Some(false);
        }
    }

    /// Ask `/props` and `/v1/models`, which never wake the server, and say
    /// whether either answered. A reply that fails keeps what the last one
    /// said, but not that the server sleeps.
    fn describe(&mut self, now: Instant, json: &dyn Fn(&str) -> Option<Value>) -> bool {
        self.described_at = Some(now);
        let props = json("/props");
        match &props {
            Some(props) => self.props = Some(Props::read(props)),
            None => {
                if let Some(props) = &mut self.props {
                    props.asleep = None;
                }
            }
        }
        let models = json("/v1/models");
        if let Some(models) = &models {
            self.catalogue = Some(Catalogue::read(models));
        }
        props.is_some() || models.is_some()
    }

    /// Whether it is time to ask again what the server serves and whether
    /// it sleeps.
    fn describe_due(&self, now: Instant, in_flight: Option<bool>) -> bool {
        let every = if in_flight.is_none() && self.asleep() == Some(true) {
            ASLEEP_RECHECK
        } else {
            DESCRIBE_EVERY
        };
        self.described_at
            .is_none_or(|at| now.saturating_duration_since(at) >= every)
    }

    /// Whether the last `/slots` read was recent enough that the server,
    /// whose idle timer it restarted, cannot have fallen asleep since.
    fn kept_awake(&self, now: Instant) -> bool {
        self.slots_read
            .is_some_and(|at| now.saturating_duration_since(at) < SLOTS_KEEP_AWAKE)
    }

    /// Whether `/slots` can be read without waking the server or keeping an
    /// idle one awake: while `/metrics` counts a request in flight; without
    /// `/metrics`, unless the server says it sleeps. Never when it is off.
    fn slots_allowed(&self, in_flight: Option<bool>) -> bool {
        if self.props.as_ref().and_then(|props| props.slots_on) == Some(false) {
            return false;
        }
        in_flight.unwrap_or(self.asleep() != Some(true))
    }

    /// The server restarted: its task numbers and counters start again, and
    /// it may serve another model.
    fn restarted(&mut self) {
        self.restarts = self.restarts.saturating_add(1);
        self.tasks.clear();
        self.drafts = None;
        self.previous = None;
        self.speculative = None;
        self.described_at = None;
    }

    /// Note each slot's task; a slot running an earlier task than before
    /// belongs to a restarted server.
    fn note_tasks(&mut self, slots: &[Slot]) {
        let restarted = slots.iter().any(|slot| {
            let (Some(id), Some(task)) = (slot.id, slot.task) else {
                return false;
            };
            self.tasks
                .iter()
                .any(|(seen, last)| *seen == id && task < *last)
        });
        if restarted {
            self.restarted();
        }
        for slot in slots {
            let (Some(id), Some(task)) = (slot.id, slot.task) else {
                continue;
            };
            match self.tasks.iter_mut().find(|(seen, _)| *seen == id) {
                Some(entry) => entry.1 = entry.1.max(task),
                None => self.tasks.push((id, task)),
            }
        }
    }

    /// Tokens per verify pass since the last reading: the pass's own token
    /// and the drafted tokens it accepted. llama-server adds a request's
    /// drafts to its counters when the request ends and prints six
    /// significant digits, so the average waits until enough passes are
    /// counted to read it to within a hundredth.
    fn tokens_per_pass(&mut self, accepted: Option<f64>, passes: Option<f64>) -> Option<f64> {
        let (Some(accepted), Some(passes)) = (accepted, passes) else {
            self.drafts = None;
            return None;
        };
        let Some((first_accepted, first_passes)) = self.drafts else {
            self.drafts = Some((accepted, passes));
            return None;
        };
        if accepted < first_accepted || passes < first_passes {
            self.drafts = Some((accepted, passes));
            return None;
        }
        let grown = passes - first_passes;
        let error = printed_error(first_passes)
            + printed_error(passes)
            + printed_error(first_accepted)
            + printed_error(accepted);
        if grown <= 0.0 || grown < 100.0 * error {
            return None;
        }
        self.drafts = Some((accepted, passes));
        Some(1.0 + (accepted - first_accepted) / grown)
    }

    /// The context the running request nearest its limit has left; with
    /// nothing running, a slot's whole context. With a unified KV cache
    /// (llama-server's default with automatic slots) running requests share
    /// one pool, so two at once each have less than this.
    fn context_left(&self, slots: Option<&[Slot]>, in_flight: Option<bool>) -> Option<u64> {
        if self.asleep() == Some(true) {
            return None;
        }
        let configured = || {
            self.catalogue
                .as_ref()
                .and_then(|catalogue| catalogue.context)
                .or_else(|| self.props.as_ref().and_then(|props| props.context))
        };
        match slots {
            Some(slots) if slots.iter().any(|slot| slot.processing) => slots
                .iter()
                .filter(|slot| slot.processing)
                .map(Slot::room)
                .collect::<Option<Vec<_>>>()?
                .into_iter()
                .min(),
            Some(slots) => slots
                .iter()
                .filter_map(|slot| slot.n_ctx)
                .max()
                .or_else(configured),
            None if in_flight == Some(false) => configured(),
            None => None,
        }
    }

    /// Whether the server drafts speculatively: what `/slots` last said of
    /// its slots, else what its counters show once requests have finished
    /// (a verify pass, or answers written without one). Unknown before.
    fn drafts(&self, counters: &Counters) -> Option<bool> {
        self.speculative.or_else(|| {
            if counters.passes? > 0.0 {
                return Some(true);
            }
            self.totals
                .filter(|totals| totals.predicted > 0.0)
                .map(|_| false)
        })
    }

    /// The model's facts from `/v1/models` and `/props`: llama.cpp gives the
    /// size of all its weights, not what one pass reads (a mixture of
    /// experts reads a few experts, and the embedding is a lookup), so the
    /// bytes per pass stay unknown.
    fn facts(&self, counters: &Counters) -> ModelFacts {
        let catalogue = self.catalogue.clone().unwrap_or_default();
        let props = self.props.clone().unwrap_or_default();
        ModelFacts {
            parameters: catalogue.parameters,
            bits_per_weight: catalogue
                .bytes
                .zip(catalogue.parameters)
                .and_then(|(bytes, parameters)| bits_per_weight(bytes, parameters)),
            quantization: catalogue.ftype.or(props.ftype),
            context_configured: catalogue.context.or(props.context),
            context_trained: catalogue.trained,
            draft: self.drafts(counters),
            runtime_version: props.build.map(|build| format!("llama.cpp {build}")),
            loaded_bytes: catalogue.bytes,
            ..ModelFacts::default()
        }
    }

    /// The requests answering now, a row each: their prompts are read, so
    /// their whole length and cache reuse are known. A prompt still being
    /// read has no row, since `/slots` does not say how long it is.
    fn requests(
        &self,
        slots: &[Slot],
        model: &str,
        rates: &[Growth],
        seconds: f64,
        observed: SystemTime,
    ) -> Vec<RequestUsage> {
        slots
            .iter()
            .filter_map(|slot| {
                let prompt = slot.prompt()?;
                let (id, task) = (slot.id?, slot.task?);
                Some(RequestUsage {
                    provider: "llama.cpp".into(),
                    model: model.into(),
                    id: request_id(task, self.restarts),
                    prompt,
                    cached: slot.cached.filter(|cached| *cached <= prompt),
                    output: slot.decoded,
                    output_tps: rates
                        .iter()
                        .find(|(s, t, _)| *s == id && *t == task)
                        .map(|(_, _, grown)| *grown as f64 / seconds),
                    completed: false,
                    queued: false,
                    prefilled: None,
                    idle_for: None,
                    ttft_ms: None,
                    observed_at: Some(observed),
                })
            })
            .collect()
    }

    /// One poll. `text` and `json` fetch a path from the server, recording
    /// how it answered.
    pub(super) fn poll(
        &mut self,
        now: Instant,
        text: &dyn Fn(&str) -> Option<String>,
        json: &dyn Fn(&str) -> Option<Value>,
    ) -> Option<LlmTelemetry> {
        // `/metrics` first: it neither wakes the server nor restarts its
        // idle timer, and says whether a request is in flight.
        let metrics = text("/metrics");
        let metrics = metrics.as_deref();
        let in_flight = metrics.and_then(|text| {
            let processing = metric_count(text, "llamacpp:requests_processing")?;
            let deferred = metric_count(text, "llamacpp:requests_deferred").unwrap_or(0);
            Some(processing > 0 || deferred > 0)
        });
        let before = self.totals;
        let averages = metrics.and_then(|text| self.averages(text));
        if before
            .zip(self.totals)
            .is_some_and(|(old, new)| new.predicted < old.predicted || new.prompt < old.prompt)
        {
            self.restarted();
        }
        let due = self.describe_due(now, in_flight);
        // Without `/metrics`, `/slots` is the only window on requests: after
        // a gap, ask `/props` first whether the server sleeps.
        let gap = in_flight.is_none() && !self.kept_awake(now) && self.asleep() != Some(true);
        let answered = (due || gap) && self.describe(now, json);
        if in_flight == Some(true) {
            // A request in flight keeps the server awake.
            self.set_awake();
        }
        let reply = if self.slots_allowed(in_flight) {
            json("/slots")
        } else {
            None
        };
        let slots = reply.as_ref().and_then(Slot::read_all);
        if let Some(slots) = &slots {
            self.slots_read = Some(now);
            self.set_awake();
            self.note_tasks(slots);
            if let Some(speculative) = slots
                .iter()
                .filter_map(|slot| slot.speculative)
                .reduce(|any, slot| any || slot)
            {
                self.speculative = Some(speculative);
            }
        }
        let progress = reply.as_ref().and_then(llama_progress);
        let (rates, seconds) = progress
            .as_deref()
            .and_then(|progress| self.growth(progress, now))
            .unwrap_or_default();
        let live = self.observe(progress, now);
        let mut result = merge_llama_metrics(reply.as_ref().and_then(parse_llama_slots), metrics);
        // The `*_tokens_seconds` gauges average the window since the last
        // scrape, and each scrape empties it: polled every second they read 0.
        // The cumulative counters' growth is the average instead.
        if let (Some(result), Some((generation, prefill))) = (result.as_mut(), averages) {
            result.generation_tps = generation;
            result.prefill_tps = prefill;
        }
        // The server's own rates average finished requests and stay at zero
        // through a long first answer; a live rate replaces them while requests run.
        if let (Some(result), Some(rate)) = (result.as_mut(), live) {
            result.generation_tps = Some(rate);
            result.generation_tps_live = true;
        }
        let asleep = self.asleep() == Some(true);
        // Asleep is idle, not a failure; a server that only described itself
        // is still there.
        let mut result = match result {
            Some(result) => result,
            None if asleep || answered => LlmTelemetry {
                source: TelemetrySource::Live,
                observed_at: Some(SystemTime::now()),
                provider: Some("llama.cpp".into()),
                status: Some(if asleep { "idle" } else { "running" }.into()),
                // Sleeping, it has nothing in flight: a request would wake it.
                active_requests: asleep.then_some(0),
                waiting_requests: asleep.then_some(0),
                ..LlmTelemetry::default()
            },
            None => return None,
        };
        let observed = result.observed_at.unwrap_or_else(SystemTime::now);
        let model = self
            .catalogue
            .as_ref()
            .and_then(|catalogue| catalogue.model.clone());
        if let Some(slots) = &slots {
            result.requests = self.requests(
                slots,
                model.as_deref().unwrap_or("unknown"),
                &rates,
                seconds,
                observed,
            );
        }
        result.model = model;
        result.kv_free_tokens = self.context_left(slots.as_deref(), in_flight);
        let counters = metrics.map(Counters::read).unwrap_or_default();
        result.tokens_per_pass = self.tokens_per_pass(counters.accepted, counters.passes);
        // Prompt tokens served from the cache, over all prompt tokens served:
        // `prompt_tokens_total` counts only those computed.
        if let (Some(computed), Some(cached)) = (counters.prompt, counters.cached) {
            let total = computed + cached;
            result.total_prompt_tokens = Some(total.round() as u64);
            result.total_cached_tokens = Some(cached.round() as u64);
            if total > 0.0 {
                result.prefix_hit_rate = Some(cached / total * 100.0);
                result.cache_efficiency = result.prefix_hit_rate;
            }
        }
        result.model_facts = self.facts(&counters);
        let reading: Vec<&Slot> = slots
            .iter()
            .flatten()
            .filter(|slot| slot.reading())
            .collect();
        let sum = |field: fn(&Slot) -> Option<u64>| {
            (!reading.is_empty())
                .then(|| {
                    reading
                        .iter()
                        .try_fold(0_u64, |sum, slot| sum.checked_add(field(slot)?))
                })
                .flatten()
        };
        result.llama = LlamaReadings {
            asleep,
            slots: self
                .props
                .as_ref()
                .and_then(|props| props.slots)
                .or_else(|| slots.as_ref().map(|slots| slots.len() as u64)),
            busy_per_decode: counters.busy_per_decode,
            reading: sum(|slot| slot.held),
            reading_cached: sum(|slot| slot.cached),
            draft_per_pass: counters
                .accepted
                .zip(counters.passes)
                .filter(|(_, passes)| *passes > 0.0)
                .map(|(accepted, passes)| 1.0 + accepted / passes),
            draft_acceptance: counters
                .accepted
                .zip(counters.drafted)
                .filter(|(accepted, drafted)| *drafted > 0.0 && accepted <= drafted)
                .map(|(accepted, drafted)| accepted / drafted),
        };
        Some(result)
    }
}

pub(super) fn merge_llama_metrics(
    slots: Option<LlmTelemetry>,
    metrics: Option<&str>,
) -> Option<LlmTelemetry> {
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

pub(super) fn metric_count(text: &str, name: &str) -> Option<u64> {
    metric(text, name)
        .filter(|n| n.fract() == 0.0 && *n < u64::MAX as f64)
        .map(|n| n as u64)
}

pub(super) fn metric(text: &str, name: &str) -> Option<f64> {
    super::native::metric_sum(text, name)
}

pub(super) fn parse_llama_slots(slots: &Value) -> Option<LlmTelemetry> {
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
                active
                    .iter()
                    .try_fold(0_u64, |sum, slot| sum.checked_add(slot_decoded(slot)?))
            })
            .flatten(),
        // A slot's context holds its answer as well as its prompt, so the
        // runtime card's prompt count stays unknown; each answering request
        // carries its own exact prompt.
        ..LlmTelemetry::default()
    })
}

#[cfg(test)]
#[path = "tests/provider_llama.rs"]
mod tests;
