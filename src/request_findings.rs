// SPDX-License-Identifier: MIT
//! What requests cost before their first token, beyond generation speed.
//! Each finding is one sentence a person can state:
//!
//! - **Cache miss.** A long prompt was read in full right after a prompt of
//!   similar length was almost all cached: at least 4,096 tokens computed
//!   with under 10% from the cache, right after a request of the same model
//!   at least 80% cached whose prompt was within a factor of two of it. It
//!   cost its prompt's reading time at this machine's usual prefill speed.
//! - **Model load.** The first token waited for the model to load: a load
//!   of a second or more that the runtime reported for the request.
//! - **Overlap.** Running two requests at once gained no throughput on this
//!   machine and raised the wait for the first token N-fold. It is judged
//!   once a session holds a minute of two requests writing at once, from
//!   three pairs or more, each request's speed then counted against its own
//!   speed alone, and ten first tokens each of requests that started alone
//!   and that started while another wrote. Two at once must have delivered
//!   under 1.1 times the work of one at a time, and the first token must
//!   have waited at least 1.5 times as long. Where the runtime says how
//!   many requests it is set to run at once and more ran, that is noted.
//! - **KV cache full.** The runtime recomputed requests for want of KV
//!   cache.
//!
//! None of them colours anything or changes the headline: colour means a
//! change in generation speed. Each is a Journal row (the overlap at most
//! once a session), the session summary adds the time they cost, and
//! Diagnostics gives the evidence. The thresholds were set on a month of
//! the owner's oMLX and Splash logs, where each fires rarely and on cases
//! that cost real time.
use crate::analysis::SAME_STORY_SECONDS;
use crate::domain::{RequestUsage, Sample};
use crate::formatting::{compact_tokens, duration_label, progress_percent};
use crate::request_history::History;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, SystemTime};

/// A prompt read in full computed at least this many tokens…
pub(crate) const FULL_READ_TOKENS: u64 = 4_096;
/// …with under this share served from the cache…
const FULL_READ_SHARE: f64 = 0.10;
/// …right after a prompt at least this share cached…
const CACHED_SHARE: f64 = 0.80;
/// …and within this factor of its length: a much shorter or longer prompt
/// is another context (an agent that compacted its history, a new task),
/// which no cache could have served.
const SIMILAR_LENGTH: u64 = 2;
/// A load shorter than this is a runtime handing over a model it already
/// holds: Ollama reports a few milliseconds with every request.
pub(crate) const LOAD_MS: u64 = 1_000;
/// Seconds of two requests writing at once, each with a known speed alone,
/// before the overlap is judged…
pub(crate) const OVERLAP_SECONDS: f64 = 60.0;
/// …from at least this many pairs of requests…
pub(crate) const OVERLAP_PAIRS: usize = 3;
/// …and first tokens of at least this many requests that started alone,
/// and as many that started while another wrote.
pub(crate) const FIRST_TOKENS: usize = 10;
/// A request's speed alone is known after this many seconds of it writing
/// with no other request writing.
const SOLO_SECONDS: f64 = 2.0;
/// Two at once gained no throughput: together under this many times the
/// work of one at a time.
const NO_GAIN: f64 = 1.1;
/// The wait for the first token must have grown at least this many fold.
const WAIT_FOLD: f64 = 1.5;
/// Polls further apart than this were not watched in between.
const MAX_GAP_SECONDS: f64 = 10.0;
/// oMLX adds a request's prompt and cached tokens to its totals as it ends;
/// a request's own count is looked for this many samples after it ended.
const SETTLE_SAMPLES: u32 = 3;
/// Requests, pairs, first tokens and prefill speeds kept: a long session
/// is judged on its recent past.
const REQUESTS_KEPT: usize = 1_024;
const PAIRS_KEPT: usize = 512;
const WAITS_KEPT: usize = 4_096;
const SPEEDS_KEPT: usize = 300;

/// The Journal states the findings are told under.
pub(crate) const CACHE_MISS: &str = "CACHE MISS";
pub(crate) const MODEL_LOAD: &str = "MODEL LOAD";
pub(crate) const KV_CACHE_FULL: &str = "KV CACHE FULL";
pub(crate) const OVERLAP: &str = "OVERLAP";

/// Each finding's Journal state, and the name a tally gives it.
pub(crate) const STATES: [(&str, &str); 4] = [
    (CACHE_MISS, "cache miss"),
    (MODEL_LOAD, "model load"),
    (KV_CACHE_FULL, "KV cache full"),
    (OVERLAP, "overlap"),
];

/// A request's identity: provider, model and id.
type Key = (String, String, String);

fn key(request: &RequestUsage) -> Key {
    (
        request.provider.clone(),
        request.model.clone(),
        request.id.clone(),
    )
}

/// How a request began: alone, while another request was writing its
/// answer, or neither (others were only reading prompts or queued, or it
/// was reported after it ended).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Start {
    Alone,
    Beside,
    Other,
}

/// A request this session has seen.
#[derive(Clone, Debug)]
struct Seen {
    key: Key,
    prompt: u64,
    /// Prompt tokens the runtime served from its cache, once known.
    cached: Option<u64>,
    /// The count is final: the runtime's totals or a finished request said it.
    settled: bool,
    start: Start,
    /// Seen queued or reading its prompt, so its wait was watched.
    watched: bool,
    /// Seconds to its first token, once it wrote one.
    first_token: Option<f64>,
    /// Tokens and seconds it wrote while no other request wrote.
    solo_tokens: f64,
    solo_seconds: f64,
    /// Samples since it ended; `None` while it runs.
    ended: Option<u32>,
    /// Judged for a prompt read in full, and its model load told.
    judged: bool,
    load_told: bool,
    /// Running, or reported after this session began: news, not history.
    fresh: bool,
}

impl Seen {
    fn is(&self, request: &RequestUsage) -> bool {
        self.key.2 == request.id && self.key.0 == request.provider && self.key.1 == request.model
    }

    /// Its speed while it wrote alone, once known.
    fn solo(&self) -> Option<f64> {
        (self.solo_seconds >= SOLO_SECONDS && self.solo_tokens > 0.0)
            .then(|| self.solo_tokens / self.solo_seconds)
    }
}

/// Two requests that wrote at once: what each wrote, over how long.
#[derive(Clone, Debug)]
struct Pair {
    a: Key,
    b: Key,
    tokens_a: f64,
    tokens_b: f64,
    seconds: f64,
}

/// A long prompt read in full right after a cached one.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FullRead {
    /// When it was told.
    pub(crate) at: String,
    pub(crate) prompt: u64,
    /// The prompt before it, and the share of that one cached.
    pub(crate) previous: u64,
    pub(crate) previous_percent: u64,
}

/// Two requests at once against one at a time, over the session.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Overlap {
    /// The work two did at once, in units of one at a time: each writer's
    /// speed over its own speed alone, added. 1 is no gain; 2 is both at
    /// their full speed.
    pub(crate) payoff: f64,
    /// Seconds the pairs wrote at once, and how many pairs.
    pub(crate) seconds: f64,
    pub(crate) pairs: usize,
    /// Median waits for the first token, beside another writing and alone,
    /// and how many requests each is over.
    pub(crate) beside: f64,
    pub(crate) alone: f64,
    pub(crate) beside_requests: usize,
    pub(crate) alone_requests: usize,
    /// Seconds requests that started beside another waited beyond the
    /// median wait of those that started alone.
    pub(crate) extra_wait: f64,
}

impl Overlap {
    /// Two at once bought nothing, and cost first tokens.
    pub(crate) fn buys_nothing(&self) -> bool {
        self.payoff < NO_GAIN && self.beside >= WAIT_FOLD * self.alone
    }
}

/// How many requests the runtime is set to run at once, against the most
/// mlxtop saw running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Limit {
    pub(crate) provider: String,
    pub(crate) setting: &'static str,
    pub(crate) configured: u64,
    pub(crate) observed: u64,
}

/// What the session's requests cost and the evidence for each, for
/// Diagnostics. Every count is of this session.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Evidence {
    /// Long prompts read in full after cached ones, the time it took to
    /// read them, and the last.
    pub(crate) full_reads: u32,
    pub(crate) full_read_seconds: f64,
    pub(crate) last_full_read: Option<FullRead>,
    /// Model loads first tokens waited for, in all, and the longest.
    pub(crate) loads: u32,
    pub(crate) load_seconds: f64,
    pub(crate) longest_load: Option<(String, f64)>,
    /// Once enough two-at-once evidence exists.
    pub(crate) overlap: Option<Overlap>,
    /// Only when more ran at once than the runtime is set to run.
    pub(crate) limit: Option<Limit>,
    /// Requests the runtime recomputed for want of KV cache, and the last time.
    pub(crate) recomputed: u64,
    pub(crate) last_recomputed: Option<String>,
}

/// The session's requests as these findings see them.
#[derive(Clone, Debug, Default)]
pub(crate) struct Findings {
    /// When the session's first sample was taken.
    started: Option<SystemTime>,
    requests: VecDeque<Seen>,
    pairs: VecDeque<Pair>,
    /// First-token waits of requests that started alone, and beside
    /// another writing.
    alone: VecDeque<f64>,
    beside: VecDeque<f64>,
    /// Live prefill speeds, and those runtimes reported per request.
    prefill_speeds: VecDeque<f64>,
    /// The most requests seen running at once, by the runtime last seen.
    observed: Option<(String, u64)>,
    overlap_told: bool,
    /// The runtime's preemption counter as last read, and when a row last
    /// said it rose.
    preemptions: Option<(String, u64)>,
    preemption_row: Option<SystemTime>,
    evidence: Evidence,
}

/// When the runtime was observed, as the session counts intervals.
fn sample_time(sample: &Sample) -> Option<SystemTime> {
    sample.llm_observed_at.or_else(|| wall_time(sample))
}

/// When the sample was taken by this machine's clock: what a reported
/// request's own time is compared with. A usage file's newest record sets
/// `llm_observed_at`, which can be long past.
fn wall_time(sample: &Sample) -> Option<SystemTime> {
    sample
        .ts
        .map(|ms| SystemTime::UNIX_EPOCH + Duration::from_millis(ms))
        .or(sample.llm_observed_at)
}

fn running(request: &RequestUsage) -> bool {
    !request.completed && !request.queued
}

fn writing(request: &RequestUsage) -> bool {
    running(request) && request.prefilled.is_none()
}

fn median(values: &VecDeque<f64>) -> Option<f64> {
    let mut sorted: Vec<f64> = values.iter().copied().collect();
    sorted.sort_by(f64::total_cmp);
    sorted.get(sorted.len() / 2).copied()
}

fn keep<T>(queue: &mut VecDeque<T>, value: T, kept: usize) {
    queue.push_back(value);
    while queue.len() > kept {
        queue.pop_front();
    }
}

/// Seconds as Journal rows give them: to a tenth under ten seconds.
pub(crate) fn seconds_label(seconds: f64) -> String {
    if seconds < 10.0 {
        format!("{seconds:.1}s")
    } else {
        duration_label(seconds)
    }
}

/// `1 request`, `n requests`.
fn requests(n: u64) -> String {
    if n == 1 {
        "1 request".into()
    } else {
        format!("{n} requests")
    }
}

/// How many requests the runtime says it runs at once, and the setting.
fn limit_of(sample: &Sample) -> Option<(u64, &'static str)> {
    match sample.llm_provider.as_str() {
        "oMLX" => sample
            .llm_max_concurrent
            .map(|limit| (limit, "max_concurrent_requests")),
        "llama.cpp" => sample.llama.slots.map(|limit| (limit, "--parallel")),
        _ => None,
    }
    .filter(|(limit, _)| *limit > 0)
}

impl Findings {
    /// What this sample adds; returns the Journal rows it gives, as state
    /// and summary. `history` has already seen the sample's requests.
    pub(crate) fn observe(
        &mut self,
        previous: Option<&Sample>,
        sample: &Sample,
        history: &History,
    ) -> Vec<(&'static str, String)> {
        let now = sample_time(sample);
        let started = *self
            .started
            .get_or_insert(wall_time(sample).unwrap_or(SystemTime::UNIX_EPOCH));
        if sample.llm_prefill_tps_live {
            if let Some(speed) = sample.llm_prefill_tps.filter(|speed| *speed > 0.0) {
                keep(&mut self.prefill_speeds, speed, SPEEDS_KEPT);
            }
        }
        if let Some(previous) = previous {
            self.end(previous, sample);
        }
        self.see(sample, started, history);
        if let Some(previous) = previous {
            self.settle(previous, sample);
            let interval = sample_time(previous)
                .zip(now)
                .and_then(|(then, now)| now.duration_since(then).ok())
                .map(|gap| gap.as_secs_f64())
                .filter(|seconds| *seconds > 0.0 && *seconds <= MAX_GAP_SECONDS);
            if let Some(seconds) = interval {
                self.writers(sample, seconds);
            }
        }
        self.observe_limit(sample);
        let overlap = self.overlap();
        self.evidence.overlap = overlap;
        let mut rows = Vec::new();
        if previous.is_some() {
            rows.extend(self.full_reads(sample));
            rows.extend(self.loads(sample));
            rows.extend(self.recomputed(sample, now));
            rows.extend(self.overlap_row(overlap));
        }
        self.preemptions = sample
            .llm_preemptions_total
            .map(|count| (sample.llm_provider.clone(), count));
        rows
    }

    /// What Diagnostics shows of the session's requests.
    pub(crate) fn evidence(&self) -> Evidence {
        self.evidence.clone()
    }

    /// Requests listed last time and not now have ended; ended ones age.
    fn end(&mut self, previous: &Sample, sample: &Sample) {
        for seen in &mut self.requests {
            if let Some(age) = seen.ended.as_mut() {
                *age = age.saturating_add(1);
            } else if previous.llm_requests.iter().any(|r| seen.is(r))
                && !sample.llm_requests.iter().any(|r| seen.is(r))
            {
                seen.ended = Some(0);
            }
        }
    }

    /// Requests seen for the first time, and what each listed one did.
    fn see(&mut self, sample: &Sample, started: SystemTime, history: &History) {
        let writers = sample.llm_requests.iter().filter(|r| writing(r)).count();
        let runners = sample.llm_requests.iter().filter(|r| running(r)).count();
        for request in &sample.llm_requests {
            let index = match self.requests.iter().position(|seen| seen.is(request)) {
                Some(index) => index,
                None => {
                    let start = if request.completed {
                        Start::Other
                    } else if writers > usize::from(writing(request)) {
                        Start::Beside
                    } else if runners == usize::from(running(request)) {
                        Start::Alone
                    } else {
                        Start::Other
                    };
                    // A finished request a runtime reported before this
                    // session began is history, not news.
                    let fresh = !request.completed
                        || request
                            .observed_at
                            .is_none_or(|at| at.duration_since(started).is_ok());
                    // Each prefill speed a runtime reported is counted once.
                    if let Some(speed) = sample
                        .llm_request_timings
                        .iter()
                        .find(|timing| timing.belongs_to(request))
                        .and_then(|timing| timing.prefill_tps)
                        .filter(|speed| *speed > 0.0)
                    {
                        keep(&mut self.prefill_speeds, speed, SPEEDS_KEPT);
                    }
                    keep(
                        &mut self.requests,
                        Seen {
                            key: key(request),
                            prompt: request.prompt,
                            cached: None,
                            settled: false,
                            start,
                            watched: false,
                            first_token: None,
                            solo_tokens: 0.0,
                            solo_seconds: 0.0,
                            ended: None,
                            judged: false,
                            load_told: false,
                            fresh,
                        },
                        REQUESTS_KEPT,
                    );
                    self.requests.len() - 1
                }
            };
            let seen = &mut self.requests[index];
            if request.completed && seen.ended.is_none() {
                seen.ended = Some(0);
            }
            if !seen.settled {
                if let Some(cached) = request.cached.filter(|cached| *cached <= request.prompt) {
                    seen.cached = Some(cached);
                    // A finished request's count is its last.
                    seen.settled = request.completed;
                }
            }
            if request.queued || request.prefilled.is_some() {
                seen.watched = true;
            }
            if seen.first_token.is_some() {
                continue;
            }
            let reported = request.ttft_ms.map(|ms| ms as f64 / 1000.0);
            let watched = (writing(request) && seen.watched)
                .then(|| history.timing(request).map(|timing| timing.first_token()))
                .flatten();
            seen.first_token = reported.or(watched);
            if let Some(wait) = seen.first_token {
                match seen.start {
                    Start::Alone => keep(&mut self.alone, wait, WAITS_KEPT),
                    Start::Beside => keep(&mut self.beside, wait, WAITS_KEPT),
                    Start::Other => {}
                }
            }
        }
    }

    /// oMLX adds each request's prompt and cached tokens to its totals as
    /// it ends: a rise of exactly one recent request's prompt is that
    /// request's count, the cached part of it included.
    fn settle(&mut self, previous: &Sample, sample: &Sample) {
        if sample.llm_provider != "oMLX" || previous.llm_provider != sample.llm_provider {
            return;
        }
        let rise = |now: Option<u64>, then: Option<u64>| now?.checked_sub(then?);
        let (Some(prompt), Some(cached)) = (
            rise(
                sample.llm_total_prompt_tokens,
                previous.llm_total_prompt_tokens,
            ),
            rise(
                sample.llm_total_cached_tokens,
                previous.llm_total_cached_tokens,
            ),
        ) else {
            return;
        };
        if prompt == 0 || cached > prompt {
            return;
        }
        let mut candidates = self.requests.iter_mut().filter(|seen| {
            seen.key.0 == sample.llm_provider
                && seen.prompt == prompt
                && !seen.settled
                && seen.ended.is_none_or(|age| age <= SETTLE_SAMPLES)
        });
        if let (Some(only), None) = (candidates.next(), candidates.next()) {
            only.cached = Some(cached);
            only.settled = true;
        }
    }

    /// Each writer's speed alone, and two writing at once.
    fn writers(&mut self, sample: &Sample, seconds: f64) {
        let rate = |request: &RequestUsage| {
            request
                .output_tps
                .filter(|tps| tps.is_finite() && *tps >= 0.0)
        };
        let writers: Vec<&RequestUsage> =
            sample.llm_requests.iter().filter(|r| writing(r)).collect();
        match writers.as_slice() {
            [only] => {
                let solo = rate(only);
                if let (Some(tps), Some(seen)) =
                    (solo, self.requests.iter_mut().find(|seen| seen.is(only)))
                {
                    seen.solo_tokens += tps * seconds;
                    seen.solo_seconds += seconds;
                }
            }
            [first, second] => {
                let (Some(rate_a), Some(rate_b)) = (rate(first), rate(second)) else {
                    return;
                };
                // One order for the pair, whichever the runtime lists first.
                let (a, rate_a, b, rate_b) = if key(first) <= key(second) {
                    (key(first), rate_a, key(second), rate_b)
                } else {
                    (key(second), rate_b, key(first), rate_a)
                };
                match self
                    .pairs
                    .iter_mut()
                    .rev()
                    .take(8)
                    .find(|pair| pair.a == a && pair.b == b)
                {
                    Some(pair) => {
                        pair.tokens_a += rate_a * seconds;
                        pair.tokens_b += rate_b * seconds;
                        pair.seconds += seconds;
                    }
                    None => keep(
                        &mut self.pairs,
                        Pair {
                            a,
                            b,
                            tokens_a: rate_a * seconds,
                            tokens_b: rate_b * seconds,
                            seconds,
                        },
                        PAIRS_KEPT,
                    ),
                }
            }
            _ => {}
        }
    }

    /// The most requests seen running at once, against the runtime's limit.
    fn observe_limit(&mut self, sample: &Sample) {
        let running = if sample.llm_requests.is_empty() {
            sample.llm_active_requests.unwrap_or(0)
        } else {
            sample.llm_requests.iter().filter(|r| running(r)).count() as u64
        };
        let observed = match self.observed.take() {
            Some((provider, most)) if provider == sample.llm_provider => most.max(running),
            _ => running,
        };
        self.observed = Some((sample.llm_provider.clone(), observed));
        self.evidence.limit = limit_of(sample)
            .filter(|(configured, _)| observed > *configured)
            .map(|(configured, setting)| Limit {
                provider: sample.llm_provider.clone(),
                setting,
                configured,
                observed,
            });
    }

    /// This machine's usual prefill speed: the median of what it measured,
    /// else the runtime's own figure.
    fn prefill_speed(&self, sample: &Sample) -> Option<f64> {
        median(&self.prefill_speeds)
            .or(sample.llm_prefill_tps)
            .filter(|speed| speed.is_finite() && *speed > 0.0)
    }

    /// Ended requests whose cached count is final, or will not be, judged
    /// against the request before them.
    fn full_reads(&mut self, sample: &Sample) -> Vec<(&'static str, String)> {
        let speed = self.prefill_speed(sample);
        let mut rows = Vec::new();
        for index in 0..self.requests.len() {
            let seen = &self.requests[index];
            let due = seen
                .ended
                .is_some_and(|age| seen.settled || age >= SETTLE_SAMPLES);
            if seen.judged || !due {
                continue;
            }
            self.requests[index].judged = true;
            let seen = &self.requests[index];
            let Some(cached) = seen.cached.filter(|_| seen.fresh && seen.prompt > 0) else {
                continue;
            };
            let computed = seen.prompt - cached;
            if computed < FULL_READ_TOKENS || cached as f64 >= FULL_READ_SHARE * seen.prompt as f64
            {
                continue;
            }
            // The latest request of the same model before it whose cached
            // count is known.
            let before = self
                .requests
                .iter()
                .take(index)
                .rev()
                .filter(|other| other.key.0 == seen.key.0 && other.key.1 == seen.key.1)
                .find_map(|other| Some((other.prompt, other.cached?)))
                .filter(|(previous, _)| *previous > 0);
            let Some((previous, previous_cached)) = before else {
                continue;
            };
            let similar = previous <= SIMILAR_LENGTH * seen.prompt
                && seen.prompt <= SIMILAR_LENGTH * previous;
            if !similar || (previous_cached as f64) < CACHED_SHARE * previous as f64 {
                continue;
            }
            let read = FullRead {
                at: sample.updated.clone(),
                prompt: seen.prompt,
                previous,
                previous_percent: progress_percent(previous_cached, previous),
            };
            let seconds = speed.map(|speed| computed as f64 / speed);
            let mut summary = format!(
                "{} · {} prompt read in full right after one of {} was {}% cached",
                seen.key.2,
                compact_tokens(read.prompt),
                compact_tokens(read.previous),
                read.previous_percent
            );
            if let Some(seconds) = seconds {
                summary.push_str(&format!(" · ≈{} of prompt reading", seconds_label(seconds)));
            }
            rows.push((CACHE_MISS, summary));
            self.evidence.full_reads += 1;
            self.evidence.full_read_seconds += seconds.unwrap_or(0.0);
            self.evidence.last_full_read = Some(read);
        }
        rows
    }

    /// Loads runtimes reported for this session's requests, once each.
    fn loads(&mut self, sample: &Sample) -> Vec<(&'static str, String)> {
        let mut rows = Vec::new();
        for timing in &sample.llm_request_timings {
            let Some(ms) = timing.load_ms.filter(|ms| *ms >= LOAD_MS) else {
                continue;
            };
            let Some(seen) = self.requests.iter_mut().find(|seen| {
                seen.key.2 == timing.id
                    && seen.key.0 == timing.provider
                    && seen.key.1 == timing.model
            }) else {
                continue;
            };
            // A reported request stays in the runtime's file: told once.
            if !seen.fresh || seen.load_told {
                continue;
            }
            seen.load_told = true;
            let seconds = ms as f64 / 1000.0;
            rows.push((
                MODEL_LOAD,
                format!(
                    "{} · first token waited {} for the model to load",
                    timing.id,
                    seconds_label(seconds)
                ),
            ));
            self.evidence.loads += 1;
            self.evidence.load_seconds += seconds;
            if self
                .evidence
                .longest_load
                .as_ref()
                .is_none_or(|(_, longest)| seconds > *longest)
            {
                self.evidence.longest_load = Some((sample.updated.clone(), seconds));
            }
        }
        rows
    }

    /// A rise in the runtime's count of requests recomputed for want of KV
    /// cache; a rise within a minute of the last row is the same story.
    fn recomputed(
        &mut self,
        sample: &Sample,
        now: Option<SystemTime>,
    ) -> Option<(&'static str, String)> {
        let (provider, before) = self.preemptions.as_ref()?;
        let count = sample.llm_preemptions_total?;
        // A counter that went back is a restarted runtime: no news.
        if *provider != sample.llm_provider || count <= *before {
            return None;
        }
        let rise = count - before;
        self.evidence.recomputed += rise;
        self.evidence.last_recomputed = Some(sample.updated.clone());
        let same_story = self.preemption_row.zip(now).is_some_and(|(row, now)| {
            now.duration_since(row)
                .is_ok_and(|gap| gap.as_secs_f64() < SAME_STORY_SECONDS)
        });
        if same_story {
            return None;
        }
        self.preemption_row = now;
        Some((
            KV_CACHE_FULL,
            format!(
                "{} {} recomputed",
                requests(rise),
                if rise == 1 { "was" } else { "were" }
            ),
        ))
    }

    /// The session's two-at-once evidence, once there is enough of it.
    fn overlap(&self) -> Option<Overlap> {
        if self.alone.len() < FIRST_TOKENS || self.beside.len() < FIRST_TOKENS {
            return None;
        }
        let solo: HashMap<&Key, f64> = self
            .requests
            .iter()
            .filter_map(|seen| Some((&seen.key, seen.solo()?)))
            .collect();
        let (mut work, mut seconds) = (0.0, 0.0);
        let mut pairs = HashSet::new();
        for pair in &self.pairs {
            let (Some(a), Some(b)) = (solo.get(&pair.a), solo.get(&pair.b)) else {
                continue;
            };
            work += pair.tokens_a / a + pair.tokens_b / b;
            seconds += pair.seconds;
            pairs.insert((&pair.a, &pair.b));
        }
        let alone = median(&self.alone)?;
        let beside = median(&self.beside)?;
        // A wait of nothing cannot grow N-fold.
        if seconds < OVERLAP_SECONDS || pairs.len() < OVERLAP_PAIRS || alone <= 0.0 {
            return None;
        }
        Some(Overlap {
            payoff: work / seconds,
            seconds,
            pairs: pairs.len(),
            beside,
            alone,
            beside_requests: self.beside.len(),
            alone_requests: self.alone.len(),
            extra_wait: self.beside.iter().map(|wait| (wait - alone).max(0.0)).sum(),
        })
    }

    /// The overlap's row, once a session, when two at once bought nothing.
    fn overlap_row(&mut self, overlap: Option<Overlap>) -> Option<(&'static str, String)> {
        if self.overlap_told {
            return None;
        }
        let overlap = overlap.filter(Overlap::buys_nothing)?;
        self.overlap_told = true;
        let mut summary = format!(
            "two requests at once gained no throughput ({:.2}× one at a time) and made first tokens wait {:.1}× as long",
            overlap.payoff,
            overlap.beside / overlap.alone
        );
        if let Some(limit) = &self.evidence.limit {
            summary.push_str(&format!(
                " · {} is set to run {} at a time",
                limit.provider, limit.configured
            ));
        }
        Some((OVERLAP, summary))
    }

    /// Seconds first tokens lost: to prompts read in full after cached
    /// ones, to model loads, and to overlap that bought nothing.
    pub(crate) fn lost(&self) -> [(&'static str, f64); 3] {
        let overlap = self
            .evidence
            .overlap
            .filter(|_| self.overlap_told)
            .map_or(0.0, |overlap| overlap.extra_wait);
        [
            ("cache misses", self.evidence.full_read_seconds),
            ("model loads", self.evidence.load_seconds),
            ("overlap", overlap),
        ]
    }

    /// Seconds first tokens lost to the finding told as `state`, as
    /// `lost` counts them; none for a full KV cache, whose cost is not
    /// counted.
    pub(crate) fn lost_to(&self, state: &str) -> Option<f64> {
        let [cache_misses, model_loads, overlap] = self.lost().map(|(_, seconds)| seconds);
        match state {
            CACHE_MISS => Some(cache_misses),
            MODEL_LOAD => Some(model_loads),
            OVERLAP => Some(overlap),
            _ => None,
        }
    }
}

/// Diagnostics' lines: the evidence for each finding, only when there is any.
pub(crate) fn lines(evidence: &Evidence) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(read) = &evidence.last_full_read {
        let time = if evidence.full_read_seconds >= 1.0 {
            format!(
                ", ≈{} of prompt reading",
                seconds_label(evidence.full_read_seconds)
            )
        } else {
            String::new()
        };
        lines.push(format!(
            "Cache misses: {} read in full right after cached ones{time}; the last at {}, {} after one of {} was {}% cached",
            if evidence.full_reads == 1 {
                "1 long prompt".to_owned()
            } else {
                format!("{} long prompts", evidence.full_reads)
            },
            read.at,
            compact_tokens(read.prompt),
            compact_tokens(read.previous),
            read.previous_percent
        ));
    }
    if let Some((at, longest)) = &evidence.longest_load {
        lines.push(format!(
            "Model loads: first tokens waited {} for {}; the longest {} at {at}",
            seconds_label(evidence.load_seconds),
            if evidence.loads == 1 {
                "1 load".to_owned()
            } else {
                format!("{} loads", evidence.loads)
            },
            seconds_label(*longest)
        ));
    }
    if let Some(overlap) = &evidence.overlap {
        lines.push(format!(
            "Two requests at once: {:.2}× the work of one at a time over {} ({} pairs); first token median {} when started beside another writing, {} alone ({} and {} requests)",
            overlap.payoff,
            seconds_label(overlap.seconds),
            overlap.pairs,
            seconds_label(overlap.beside),
            seconds_label(overlap.alone),
            overlap.beside_requests,
            overlap.alone_requests
        ));
    }
    if let Some(limit) = &evidence.limit {
        lines.push(format!(
            "{} is set to run {} at a time ({}), yet {} ran at once",
            limit.provider,
            requests(limit.configured),
            limit.setting,
            limit.observed
        ));
    }
    if evidence.recomputed > 0 {
        lines.push(format!(
            "KV cache full: {} recomputed this session{}",
            requests(evidence.recomputed),
            evidence
                .last_recomputed
                .as_ref()
                .map(|at| format!(", the last at {at}"))
                .unwrap_or_default()
        ));
    }
    lines
}

/// The session summary's lines: what first tokens lost, by cause, when
/// anything was lost.
pub(crate) fn summary(findings: &Findings) -> Vec<String> {
    let lost: Vec<(&str, f64)> = findings
        .lost()
        .into_iter()
        .filter(|(_, seconds)| *seconds >= 1.0)
        .collect();
    if lost.is_empty() {
        return Vec::new();
    }
    let total: f64 = lost.iter().map(|(_, seconds)| seconds).sum();
    let mut lines = vec![format!(
        "Time lost before first tokens: {}",
        duration_label(total)
    )];
    for (cause, seconds) in lost {
        lines.push(format!("  {cause:<26} {}", duration_label(seconds)));
    }
    lines
}

#[cfg(test)]
#[path = "tests/request_findings.rs"]
mod tests;
