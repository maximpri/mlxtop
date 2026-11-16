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

/// An axis top for a byte rate, round in the unit it is shown in: tenths of
/// a KiB, MiB or GiB, so a 1 MiB/s floor tops out at 1.5 MiB/s, not the
/// 1.4 MiB/s a round count of bytes gives.
pub(crate) fn ceiling_bytes(peak: u64, minimum: u64) -> u64 {
    let padded = peak.saturating_add(peak.div_ceil(10)).max(minimum);
    let mut unit = 1_u64;
    while padded / unit >= 1024 && unit < 1 << 40 {
        unit *= 1024;
    }
    if unit == 1 {
        return ceiling(peak, minimum);
    }
    let tenths = (u128::from(padded) * 10).div_ceil(u128::from(unit)) as u64;
    let step = nice_step(tenths.div_ceil(5));
    let top = u128::from(tenths.div_ceil(step).saturating_mul(step)) * u128::from(unit) / 10;
    u64::try_from(top).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[path = "tests/chart_scale.rs"]
mod tests;
