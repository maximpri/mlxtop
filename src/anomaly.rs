// SPDX-License-Identifier: MIT
//! Anomaly detection: one rule for every signal.
//!
//! A signal has a usual value (the middle of its recent samples) and a usual
//! spread (how far it ordinarily strays from that). A sample is unusual when
//! it is further from the usual value than the signal ordinarily strays, by
//! an amount that matters. A signal that jumps about has a wide spread and
//! is rarely unusual; a steady one has a narrow spread and a small change
//! shows. Nothing here knows which signal it is judging.
use std::collections::VecDeque;

/// Samples a usual value is taken over: about five minutes of generation,
/// long enough to span several requests, whose content alone moves speed.
pub(crate) const WINDOW: usize = 300;
/// Samples needed before a signal has a usual value at all.
pub(crate) const LEARN_SAMPLES: usize = 8;
/// How many usual spreads from the usual value a sample must be. 3.5 is the
/// customary cut for a robust (median-based) deviation score.
pub(crate) const DEVIATIONS: f64 = 3.5;

/// What a signal has been doing: its recent samples, unusual ones left out.
#[derive(Clone, Default)]
pub(crate) struct Usual {
    values: VecDeque<f64>,
}

impl Usual {
    pub(crate) fn learn(&mut self, value: f64) {
        if !value.is_finite() {
            return;
        }
        self.values.push_back(value);
        while self.values.len() > WINDOW {
            self.values.pop_front();
        }
    }

    /// Starts over from `values`: what was unusual has held long enough to
    /// be how the signal now runs.
    pub(crate) fn relearn(&mut self, values: impl IntoIterator<Item = f64>) {
        self.values.clear();
        for value in values {
            self.learn(value);
        }
    }

    /// The usual value: the middle of the recent samples, once there are
    /// enough to say.
    pub(crate) fn level(&self) -> Option<f64> {
        (self.values.len() >= LEARN_SAMPLES).then(|| median(self.values.iter().copied().collect()))
    }

    /// How far the signal ordinarily strays from its usual value: the median
    /// absolute deviation, scaled to match a standard deviation, so a single
    /// burst or stall does not widen it.
    pub(crate) fn spread(&self) -> f64 {
        let Some(level) = self.level() else {
            return 0.0;
        };
        median(
            self.values
                .iter()
                .map(|value| (value - level).abs())
                .collect(),
        ) * 1.4826
    }

    /// The usual value and how far from it a sample must be to be unusual:
    /// `DEVIATIONS` spreads, and never less than `share` of the usual value
    /// or `least` outright, so a change too small to matter is not reported
    /// on a signal that happens to be very steady.
    pub(crate) fn range(&self, share: f64, least: f64) -> Option<(f64, f64)> {
        let level = self.level()?;
        Some((
            level,
            (DEVIATIONS * self.spread())
                .max(share * level.abs())
                .max(least),
        ))
    }

    /// The recent samples, oldest first.
    pub(crate) fn values(&self) -> impl Iterator<Item = f64> + '_ {
        self.values.iter().copied()
    }

    /// The usual value, when `value` is unusually far above it.
    pub(crate) fn above(&self, value: f64, share: f64, least: f64) -> Option<f64> {
        self.range(share, least)
            .filter(|(level, margin)| value - level >= *margin)
            .map(|(level, _)| level)
    }

    /// The usual value, when `value` is unusually far below it.
    pub(crate) fn below(&self, value: f64, share: f64, least: f64) -> Option<f64> {
        self.range(share, least)
            .filter(|(level, margin)| level - value >= *margin)
            .map(|(level, _)| level)
    }
}

pub(crate) fn median(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

#[cfg(test)]
#[path = "tests/anomaly.rs"]
mod tests;
