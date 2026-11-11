// SPDX-License-Identifier: MIT
//! What a session amounted to: how long the model worked and where its
//! requests' time went, how much speed each cause cost, what the prompt
//! cache saved, and the energy per token. A finding scrolls out of the
//! window; this keeps the total, so the change worth making is the one at
//! the top.
use crate::domain::{CorrelationCause, Phase, Sample};
use crate::formatting::{compact_tokens, duration_label};
use crate::request_history::{History, Timing};
use std::collections::BTreeMap;
use std::time::SystemTime;

/// Polls further apart than this were not watched in between.
const MAX_GAP_SECONDS: f64 = 10.0;

#[derive(Clone, Default)]
pub(crate) struct Session {
    pub(crate) started: Option<SystemTime>,
    last: Option<SystemTime>,
    /// Seconds watched while an answer was being written, or a prompt read.
    pub(crate) generating: f64,
    pub(crate) prefilling: f64,
    /// Tokens written, from the measured speed over each interval.
    pub(crate) tokens: f64,
    /// Energy the machine drew while the model worked, in joules.
    pub(crate) joules: f64,
    /// Seconds of work a slowdown or stall cost, by its reason: the share
    /// of each interval the model ran below its usual speed.
    pub(crate) lost: BTreeMap<&'static str, f64>,
}

impl Session {
    /// Count one sample.
    pub(crate) fn observe(&mut self, sample: &Sample) {
        let now = sample.llm_observed_at.unwrap_or_else(SystemTime::now);
        self.started.get_or_insert(now);
        let seconds = self
            .last
            .replace(now)
            .and_then(|last| now.duration_since(last).ok())
            .map_or(0.0, |gap| gap.as_secs_f64());
        if seconds <= 0.0 || seconds > MAX_GAP_SECONDS {
            return;
        }
        let insight = &sample.correlation;
        let writing = sample.llm_generation_tps_live
            && sample.llm_generation_tps.is_some_and(|tps| tps > 0.0);
        let stalled = insight.is_material_drop() && insight.cause == CorrelationCause::Stall;
        let reading = !writing
            && sample.llm_prefill_tps_live
            && sample.llm_prefill_tps.is_some_and(|tps| tps > 0.0);
        if writing || stalled {
            self.generating += seconds;
            self.tokens += sample.llm_generation_tps.unwrap_or(0.0) * seconds;
        } else if reading {
            self.prefilling += seconds;
        }
        if writing || stalled || reading {
            if let Some(milliwatts) = sample.system_power_mw {
                self.joules += milliwatts as f64 / 1000.0 * seconds;
            }
        }
        if !insight.is_material_drop() {
            return;
        }
        let speed = match insight.phase {
            Phase::Generation => sample.llm_generation_tps,
            Phase::Prefill => sample.llm_prefill_tps,
        };
        let share = match (insight.cause, insight.episode, speed) {
            (CorrelationCause::Stall, _, _) => 1.0,
            (_, Some(episode), Some(speed)) if episode.reference > 0.0 => {
                (1.0 - speed / episode.reference).clamp(0.0, 1.0)
            }
            _ => 0.0,
        };
        if share > 0.0 {
            *self.lost.entry(insight.cause.label()).or_default() += share * seconds;
        }
    }

    /// Joules per token written, when power was read.
    pub(crate) fn joules_per_token(&self) -> Option<f64> {
        (self.joules > 0.0 && self.tokens >= 1.0).then(|| self.joules / self.tokens)
    }
}

/// The summary as lines, most useful first.
pub(crate) fn lines(
    session: &Session,
    requests: &History,
    sample: &Sample,
    saved: Option<f64>,
) -> Vec<String> {
    let mut lines = Vec::new();
    let watched = session
        .started
        .zip(session.last)
        .and_then(|(start, end)| end.duration_since(start).ok())
        .map_or(0.0, |gap| gap.as_secs_f64());
    lines.push(format!(
        "Watched {} · writing {} · reading prompts {}",
        duration_label(watched),
        duration_label(session.generating),
        duration_label(session.prefilling)
    ));
    let mut lost: Vec<(&str, f64)> = session.lost.iter().map(|(k, v)| (*k, *v)).collect();
    lost.sort_by(|a, b| b.1.total_cmp(&a.1));
    if lost.is_empty() {
        lines.push("Speed lost: none · no slowdown or stall".into());
    } else {
        let total: f64 = lost.iter().map(|(_, seconds)| seconds).sum();
        lines.push(format!("Speed lost: {} of work", duration_label(total)));
        for (cause, seconds) in lost {
            lines.push(format!("  {:<26} {}", cause, duration_label(seconds)));
        }
    }
    let done: Vec<(&crate::domain::RequestUsage, Timing)> = requests
        .entries
        .iter()
        .filter(|entry| entry.timing.total() > 0.0)
        .map(|entry| (&entry.usage, entry.timing))
        .collect();
    if !done.is_empty() {
        let sum = done.iter().fold(Timing::default(), |sum, (_, t)| Timing {
            queued: sum.queued + t.queued,
            prefill: sum.prefill + t.prefill,
            decode: sum.decode + t.decode,
        });
        let total = sum.total().max(f64::EPSILON);
        let share = |part: f64| part / total * 100.0;
        lines.push(format!(
            "Requests: {} · time went to queue {:.0}% · prompt {:.0}% · answer {:.0}%",
            done.len(),
            share(sum.queued),
            share(sum.prefill),
            share(sum.decode)
        ));
        let mut waits: Vec<f64> = done.iter().map(|(_, t)| t.first_token()).collect();
        waits.sort_by(f64::total_cmp);
        lines.push(format!(
            "  first token: median {} · slowest {}",
            duration_label(waits[waits.len() / 2]),
            duration_label(*waits.last().unwrap_or(&0.0))
        ));
        let mut slowest: Vec<(&crate::domain::RequestUsage, Timing)> = done
            .iter()
            .filter(|(usage, t)| t.decode > 0.0 && usage.output.unwrap_or(0) > 0)
            .cloned()
            .collect();
        slowest.sort_by(|a, b| {
            let speed = |(usage, t): &(&crate::domain::RequestUsage, Timing)| {
                usage.output.unwrap_or(0) as f64 / t.decode
            };
            speed(a).total_cmp(&speed(b))
        });
        for (usage, timing) in slowest.iter().take(3) {
            lines.push(format!(
                "  slow: {} · {:.0} tok/s · {} prompt · {}",
                usage.id,
                usage.output.unwrap_or(0) as f64 / timing.decode,
                compact_tokens(usage.prompt),
                crate::collector::timing_label(timing)
            ));
        }
    }
    if let Some(hit) = sample.llm_cache_efficiency {
        lines.push(match saved {
            Some(seconds) if seconds >= 1.0 => format!(
                "KV cache: {hit:.0}% of prompt tokens served from cache since the runtime started · ≈{} of prefill saved while watched",
                duration_label(seconds)
            ),
            _ => format!(
                "KV cache: {hit:.0}% of prompt tokens served from cache since the runtime started"
            ),
        });
    }
    if let Some(joules) = session.joules_per_token() {
        lines.push(format!(
            "Energy: {:.2} J per token written · {:.1} Wh while working",
            joules,
            session.joules / 3600.0
        ));
    }
    lines
}

#[cfg(test)]
#[path = "tests/session.rs"]
mod tests;
