// SPDX-License-Identifier: MIT
//! How close decode runs to what the chip's memory can feed it, counted per
//! verify pass. A pass of the model reads its weights once, the KV cache of
//! the context it decodes and the sequence's fixed state, and emits one
//! token, or with speculative decoding the drafted tokens it accepts and
//! one of its own. Counted per token instead, a server whose draft lands
//! 7.8 tokens a pass reads 450% of its chip's peak, which cannot be.
//!
//! Every figure is an estimate from the chip's published peak and the
//! model's own files, never coloured, and unknown (never zero, never above
//! 100%) when an input is missing or the inputs do not add up. The draft's
//! own reads are left out: a runtime may hold it in another format than its
//! files (Splash loads a bf16 draft as 4-bit), so what it reads is not known.
use crate::chip_facts::{self, Bandwidth};
use crate::domain::Sample;
use std::collections::VecDeque;

/// One sample's decode, counted per verify pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Estimate {
    /// The chip's published peak.
    pub(crate) peak: Bandwidth,
    /// Generation speed, tokens a second.
    pub(crate) speed: f64,
    /// Tokens a verify pass emits: the runtime's count with a draft, one
    /// without.
    pub(crate) tokens_per_pass: f64,
    /// A draft proposes the tokens a pass verifies.
    pub(crate) draft: bool,
    /// Verify passes a second: the speed over tokens per pass.
    pub(crate) passes: f64,
    /// What one pass reads: the weights, the context's KV cache and the
    /// sequence's fixed state.
    pub(crate) bytes_per_pass: u64,
    /// The context the pass decodes: prompt and output so far.
    pub(crate) context: u64,
    /// The share of the peak those passes read: above 0, at most 1.
    pub(crate) share: f64,
}

impl Estimate {
    /// The speed this machine could not exceed without a draft: one token a
    /// pass, at the chip's full peak.
    pub(crate) fn ceiling(&self) -> f64 {
        self.peak.gb_s * 1e9 / self.bytes_per_pass as f64
    }
}

/// Decode per verify pass in `sample`, while one request writes its answer
/// with nothing beside it: how a pass is shared between sequences is the
/// runtime's own. Unknown for a remote runtime or a model partly in CPU
/// memory; when the chip's peak, any byte a pass reads or the context is
/// unknown; when a draft runs and the runtime does not say how many tokens
/// a pass emits, or runs beside an MoE model (a pass verifying several
/// tokens reads the experts each of them routes to); and when the figure
/// comes out above the peak, which means an input is wrong.
pub(crate) fn of_sample(sample: &Sample) -> Option<Estimate> {
    if sample.llm_remote || !sample.llm_fresh() || !sample.llm_generation_tps_live {
        return None;
    }
    let speed = sample
        .llm_generation_tps
        .filter(|speed| speed.is_finite() && *speed > 0.0)?;
    // The layers in CPU memory read at the CPU's pace, not the chip's.
    if sample.llm_model_offloaded.is_some_and(|bytes| bytes > 0) {
        return None;
    }
    let peak = chip_facts::of_sample(sample)?;
    let (tokens_per_pass, draft) = tokens_per_pass(sample)?;
    let facts = &sample.model;
    if draft && facts.experts.is_some() {
        return None;
    }
    let context = context(sample)?;
    let bytes_per_pass = facts
        .bytes_per_pass
        .filter(|bytes| *bytes > 0)?
        .checked_add(facts.kv_bytes_per_token()?.checked_mul(context)?)?
        .checked_add(facts.fixed_state_bytes?)?;
    let passes = speed / tokens_per_pass;
    let share = passes * bytes_per_pass as f64 / (peak.gb_s * 1e9);
    (share > 0.0 && share <= 1.0).then_some(Estimate {
        peak,
        speed,
        tokens_per_pass,
        draft,
        passes,
        bytes_per_pass,
        context,
        share,
    })
}

/// Tokens a verify pass emits, and whether a draft proposes them: the
/// runtime's own count, else one for a model known to decode without a
/// draft. Unknown while a draft runs and the runtime does not say.
fn tokens_per_pass(sample: &Sample) -> Option<(f64, bool)> {
    let drafting = sample.model.draft == Some(true) || sample.llm_draft_acceptance.is_some();
    match sample.llm_tokens_per_pass {
        // A pass emits at least the token the model adds itself.
        Some(tokens) if tokens.is_finite() && tokens >= 1.0 => {
            Some((tokens, drafting || tokens > 1.0))
        }
        Some(_) => None,
        None if drafting => None,
        None => (sample.model.draft == Some(false)).then_some((1.0, false)),
    }
}

/// The context of the one sequence decoding: its prompt and output so far.
/// Unknown when more than one request runs, or one is reading its prompt.
fn context(sample: &Sample) -> Option<u64> {
    let running: Vec<_> = sample
        .llm_requests
        .iter()
        .filter(|request| !request.completed && !request.queued)
        .collect();
    match running.as_slice() {
        // A runtime that lists no requests: one active, and its tokens.
        [] => (sample.llm_active_requests == Some(1))
            .then(|| crate::analysis::llm_context_tokens(sample))
            .flatten()
            .filter(|tokens| *tokens > 0),
        [only] if only.prefilled.is_none() && only.prompt > 0 => {
            Some(only.prompt.saturating_add(only.output.unwrap_or(0)))
        }
        _ => None,
    }
}

/// What Diagnostics says: the share of the peak decode reads, and the best
/// it has reached here for this work when that is known.
pub(crate) fn share_line(estimate: &Estimate, best: Option<f64>) -> String {
    format!("Decode estimate: {}.", share_text(estimate, best))
}

/// The share in words, without a label: `59% of 410 GB/s per verify pass,
/// draft excluded; best 61%`.
pub(crate) fn share_text(estimate: &Estimate, best: Option<f64>) -> String {
    let mut text = format!(
        "{:.0}% of {}",
        estimate.share * 100.0,
        estimate.peak.label()
    );
    if estimate.draft {
        text.push_str(" per verify pass, draft excluded");
    }
    if let Some(best) = best {
        text.push_str(&format!("; best {:.0}%", best * 100.0));
    }
    text
}

/// With a draft, what it pays: the tokens a pass emits, and the speed this
/// machine could not exceed without it. `None` without a draft.
pub(crate) fn draft_text(estimate: &Estimate) -> Option<String> {
    estimate.draft.then(|| {
        format!(
            "{:.1} tokens per pass; without it, at most an estimated {}",
            estimate.tokens_per_pass,
            speed(estimate.ceiling())
        )
    })
}

/// A speed as a summary says it: whole tok/s from 10 up, a decimal below.
fn speed(tokens_per_second: f64) -> String {
    if tokens_per_second >= 10.0 {
        format!("{tokens_per_second:.0} tok/s")
    } else {
        format!("{tokens_per_second:.1} tok/s")
    }
}

/// Readings kept: about fifty minutes of decode at one a second.
const READINGS: usize = 3000;
/// A best, or a band's usual figures, needs this many readings.
const ENOUGH: usize = 20;
/// The best is the share nine readings in ten stay under: a high
/// percentile, so one lucky poll does not set it.
const BEST: f64 = 0.9;
/// Decode is near its best within a tenth of it.
const NEAR: f64 = 0.9;
/// Decode now is the middle of the latest readings, so one jumpy poll
/// neither puts the note up nor takes it down.
const NOW: usize = 5;
/// Bands of context the session summary splits decode by, as the server's
/// own tables did: under 4k, then by factors of four.
const BANDS: [(&str, u64); 5] = [
    ("under 4k", 4_096),
    ("4k–16k", 16_384),
    ("16k–64k", 65_536),
    ("64k–256k", 262_144),
    ("256k+", u64::MAX),
];

/// One sample's decode, kept while generation ran at its usual speed.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Reading {
    context: u64,
    speed: f64,
    tokens_per_pass: f64,
    share: f64,
}

/// What decode reached this session for the model being served, outside
/// slowdowns: the fairest measure of what this machine can do with it,
/// since the published peak is never reached.
#[derive(Clone, Debug, Default)]
pub(crate) struct Record {
    /// The runtime and model the readings are of.
    model: Option<(String, String)>,
    /// The chip's peak and whether a draft ran, as of the latest reading:
    /// how the summary says its figures.
    peak: Option<Bandwidth>,
    draft: bool,
    readings: VecDeque<Reading>,
}

impl Record {
    /// Count one sample: its decode, when generation kept its usual speed.
    /// A new model starts the record over.
    pub(crate) fn observe(&mut self, sample: &Sample) {
        let insight = &sample.correlation;
        // What a slowed machine does is not what it can do.
        if insight.slow || insight.episode.is_some() || insight.is_material_drop() {
            return;
        }
        let Some(estimate) = of_sample(sample) else {
            return;
        };
        if !self.serves(sample) {
            self.readings.clear();
            self.model = Some((sample.llm_provider.clone(), sample.llm_model.clone()));
        }
        self.peak = Some(estimate.peak);
        self.draft = estimate.draft;
        self.readings.push_back(Reading {
            context: estimate.context,
            speed: estimate.speed,
            tokens_per_pass: estimate.tokens_per_pass,
            share: estimate.share,
        });
        while self.readings.len() > READINGS {
            self.readings.pop_front();
        }
    }

    /// Whether the readings are of the runtime and model `sample` serves.
    fn serves(&self, sample: &Sample) -> bool {
        self.model.as_ref().is_some_and(|(provider, model)| {
            *provider == sample.llm_provider && *model == sample.llm_model
        })
    }

    /// The model's readings at a context within a factor of two of
    /// `context`, oldest first: similar work, as the usual speed compares.
    fn similar(&self, sample: &Sample, context: u64) -> Vec<&Reading> {
        if !self.serves(sample) {
            return Vec::new();
        }
        let context = context.max(1);
        self.readings
            .iter()
            .filter(|reading| {
                let earlier = reading.context.max(1);
                earlier <= context.saturating_mul(2) && context <= earlier.saturating_mul(2)
            })
            .collect()
    }

    /// The best share decode has reached here for work like `estimate`'s:
    /// the share nine readings in ten of similar work stay under.
    pub(crate) fn best(&self, sample: &Sample, estimate: &Estimate) -> Option<f64> {
        let shares: Vec<f64> = self
            .similar(sample, estimate.context)
            .iter()
            .map(|reading| reading.share)
            .collect();
        if shares.len() < ENOUGH {
            return None;
        }
        percentile(shares, BEST)
    }

    /// Whether decode runs near that best: the middle of its latest readings
    /// of similar work within a tenth of it.
    pub(crate) fn near_best(&self, sample: &Sample, estimate: &Estimate) -> bool {
        let Some(best) = self.best(sample, estimate) else {
            return false;
        };
        let latest: Vec<f64> = self
            .similar(sample, estimate.context)
            .iter()
            .rev()
            .take(NOW)
            .map(|reading| reading.share)
            .collect();
        percentile(latest, 0.5).is_some_and(|now| now >= NEAR * best)
    }

    /// The session's decode by context, for its summary: for each band with
    /// enough readings, the usual share of the peak, speed and, with a
    /// draft, tokens per pass. Nothing without them.
    pub(crate) fn summary(&self) -> Vec<String> {
        let Some(peak) = self.peak else {
            return Vec::new();
        };
        let mut lines = Vec::new();
        let mut from = 0;
        for (label, below) in BANDS {
            let band: Vec<&Reading> = self
                .readings
                .iter()
                .filter(|reading| (from..below).contains(&reading.context))
                .collect();
            from = below;
            if band.len() < ENOUGH {
                continue;
            }
            let usual = |value: fn(&Reading) -> f64| {
                percentile(band.iter().copied().map(value).collect(), 0.5).unwrap_or_default()
            };
            let mut row = format!(
                "  {label:<10}{:.0}% of {} · usual {}",
                usual(|reading| reading.share) * 100.0,
                peak.label(),
                speed(usual(|reading| reading.speed))
            );
            if self.draft {
                row.push_str(&format!(
                    " · {:.1} tokens per pass",
                    usual(|reading| reading.tokens_per_pass)
                ));
            }
            lines.push(row);
        }
        if !lines.is_empty() {
            lines.insert(
                0,
                if self.draft {
                    "Decode per verify pass by context (estimate, draft excluded):"
                } else {
                    "Decode by context (estimate):"
                }
                .into(),
            );
        }
        lines
    }
}

/// The `p` quantile of `values`, between the two nearest when it falls
/// between them; `None` without values.
fn percentile(mut values: Vec<f64>, p: f64) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    let last = values.len().checked_sub(1)?;
    let at = last as f64 * p.clamp(0.0, 1.0);
    let (low, high) = (at.floor() as usize, at.ceil() as usize);
    Some(values[low] + (values[high] - values[low]) * (at - low as f64))
}

#[cfg(test)]
#[path = "tests/efficiency.rs"]
mod tests;
