// SPDX-License-Identifier: MIT
//! Compact operator view of sampled request load; never a billing ledger.
use crate::domain;
use crate::domain::Sample;
use std::collections::VecDeque;
use std::time::SystemTime;

pub(crate) const HISTORY_LIMIT: usize = 240;

#[derive(Clone)]
pub(crate) struct Entry {
    pub(crate) number: u64,
    pub(crate) usage: domain::RequestUsage,
    pub(crate) last_seen: SystemTime,
    pub(crate) output_speed: Option<OutputSpeed>,
    /// Where the request's time went, as polls saw it.
    pub(crate) timing: Timing,
}

/// Seconds a request spent waiting in the queue, reading its prompt and
/// writing its answer. Each poll's interval goes to the phase the request
/// was in at the start of it, so each part is accurate to one poll.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Timing {
    pub(crate) queued: f64,
    pub(crate) prefill: f64,
    pub(crate) decode: f64,
}

impl Timing {
    /// Time to its first token: waiting, then reading the prompt.
    pub(crate) fn first_token(&self) -> f64 {
        self.queued + self.prefill
    }

    pub(crate) fn total(&self) -> f64 {
        self.queued + self.prefill + self.decode
    }

    fn add(&mut self, usage: &domain::RequestUsage, seconds: f64) {
        if !seconds.is_finite() || seconds <= 0.0 {
            return;
        }
        if usage.queued {
            self.queued += seconds;
        } else if usage.prefilled.is_some() {
            self.prefill += seconds;
        } else {
            self.decode += seconds;
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct OutputSpeed {
    pub(crate) tps: f64,
    pub(crate) observed_at: SystemTime,
    pub(crate) completed: bool,
}

pub(crate) fn observed_output_speed(
    usage: &domain::RequestUsage,
    observed_at: SystemTime,
) -> Option<OutputSpeed> {
    let tps = usage
        .output_tps
        .filter(|tps| tps.is_finite() && *tps >= 0.0)?;
    Some(OutputSpeed {
        tps,
        observed_at,
        completed: usage.completed,
    })
}

#[derive(Clone, Default)]
pub(super) struct History {
    pub(crate) entries: VecDeque<Entry>,
    pub(crate) next_number: u64,
}

pub(crate) fn same_request(a: &domain::RequestUsage, b: &domain::RequestUsage) -> bool {
    a.id == b.id && a.provider == b.provider && a.model == b.model
}

impl History {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn latest_for(&self, sample: &Sample) -> Option<(&domain::RequestUsage, SystemTime)> {
        self.entries
            .iter()
            .rev()
            .find(|entry| {
                entry.usage.provider == sample.llm_provider && entry.usage.model == sample.llm_model
            })
            .map(|entry| (&entry.usage, entry.last_seen))
    }

    /// Where a request's time went, once polls have seen it.
    pub fn timing(&self, request: &domain::RequestUsage) -> Option<Timing> {
        self.entries
            .iter()
            .rev()
            .find(|entry| same_request(&entry.usage, request))
            .map(|entry| entry.timing)
            .filter(|timing| timing.total() > 0.0)
    }

    pub fn observe(&mut self, requests: &[domain::RequestUsage]) {
        for usage in requests {
            if let Some(entry) = self
                .entries
                .iter_mut()
                .find(|entry| same_request(&entry.usage, usage))
            {
                // Retained provider responses and repeated file reads do not
                // refresh an observation's age.
                let seen = usage.observed_at.unwrap_or(entry.last_seen);
                if !entry.usage.completed {
                    let elapsed = seen
                        .duration_since(entry.last_seen)
                        .map_or(0.0, |gap| gap.as_secs_f64());
                    entry.timing.add(&entry.usage, elapsed);
                }
                entry.last_seen = seen;
                if let Some(speed) = observed_output_speed(usage, entry.last_seen) {
                    entry.output_speed = Some(speed);
                }
                entry.usage = usage.clone();
            } else {
                self.next_number = self.next_number.saturating_add(1);
                let last_seen = usage.observed_at.unwrap_or_else(SystemTime::now);
                self.entries.push_back(Entry {
                    number: self.next_number,
                    usage: usage.clone(),
                    last_seen,
                    output_speed: observed_output_speed(usage, last_seen),
                    timing: Timing::default(),
                });
                while self.entries.len() > HISTORY_LIMIT {
                    self.entries.pop_front();
                }
            }
        }
    }
}
