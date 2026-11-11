// SPDX-License-Identifier: MIT
//! Human-readable axes derived from visible observations, never model limits.

pub(crate) fn nice_step(value: u64) -> u64 {
    let value = value.max(1);
    let power = 10_u64.pow(value.ilog10());
    [1_u64, 2, 5, 10]
        .into_iter()
        .map(|factor| power.saturating_mul(factor))
        .find(|step| *step >= value)
        .unwrap_or(u64::MAX)
}

pub(crate) fn ceiling(peak: u64, minimum: u64) -> u64 {
    let padded = peak.saturating_add(peak.div_ceil(10)).max(minimum);
    let step = nice_step(padded.div_ceil(5));
    padded.div_ceil(step).saturating_mul(step)
}

#[cfg(test)]
#[path = "tests/chart_scale.rs"]
mod tests;
