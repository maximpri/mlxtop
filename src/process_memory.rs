// SPDX-License-Identifier: MIT
//! OS accounting for one process, independent of provider/allocator telemetry.
use crate::domain::Sample;
use crate::formatting::{bytes, signed_rate};
use crate::history::signed_rate_bytes;
use std::time::Instant;

#[derive(Clone)]
pub(super) struct Reading {
    pub pid: u32,
    pub started: u64,
    pub resident: u64,
    pub footprint: u64,
    pub peak: u64,
    /// Bytes the process has read from and written to storage since it
    /// started, through the file system (not paging or memory maps).
    pub disk_read: u64,
    pub disk_written: u64,
    pub at: Instant,
}

#[cfg(target_os = "macos")]
pub(super) fn read(pid: u32) -> Option<Reading> {
    use libproc::pid_rusage::{pidrusage, RUsageInfoV4};
    let pid_i32 = i32::try_from(pid).ok().filter(|pid| *pid > 0)?;
    let usage = pidrusage::<RUsageInfoV4>(pid_i32).ok()?;
    Some(Reading {
        pid,
        started: usage.ri_proc_start_abstime,
        resident: usage.ri_resident_size,
        footprint: usage.ri_phys_footprint,
        peak: usage.ri_lifetime_max_phys_footprint,
        disk_read: usage.ri_diskio_bytesread,
        disk_written: usage.ri_diskio_byteswritten,
        at: Instant::now(),
    })
}

#[cfg(not(target_os = "macos"))]
pub(super) fn read(_pid: u32) -> Option<Reading> {
    None
}

pub(super) fn growth(current: &Reading, previous: Option<&Reading>) -> Option<i64> {
    let previous = previous?;
    if current.pid != previous.pid || current.started != previous.started {
        return None;
    }
    let elapsed = current.at.checked_duration_since(previous.at)?;
    if elapsed.is_zero() {
        return None;
    }
    Some(signed_rate_bytes(
        current.footprint,
        previous.footprint,
        elapsed,
    ))
}

/// Storage read and write rates in bytes per second since the previous
/// reading of the same process.
pub(super) fn disk_rates(current: &Reading, previous: Option<&Reading>) -> Option<(u64, u64)> {
    let previous = previous?;
    if current.pid != previous.pid || current.started != previous.started {
        return None;
    }
    let elapsed = current
        .at
        .checked_duration_since(previous.at)?
        .as_secs_f64();
    if elapsed <= 0.0 {
        return None;
    }
    let rate = |now: u64, then: u64| (now.saturating_sub(then) as f64 / elapsed).round() as u64;
    Some((
        rate(current.disk_read, previous.disk_read),
        rate(current.disk_written, previous.disk_written),
    ))
}

pub(super) fn detail(sample: &Sample) -> String {
    sample
        .process_memory
        .as_ref()
        .map(|reading| {
            format!(
                "peak {} · growth {}",
                bytes(reading.peak),
                sample
                    .process_memory_growth
                    .map(signed_rate)
                    .unwrap_or_else(|| "—".into())
            )
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "tests/process_memory.rs"]
mod tests;
