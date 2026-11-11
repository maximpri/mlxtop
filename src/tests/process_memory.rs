use crate::test_support::*;
// SPDX-License-Identifier: MIT
use super::*;

#[test]
fn growth_tracks_same_process_and_preserves_decreases() {
    let first = Reading {
        pid: 42,
        started: 1,
        resident: 100,
        footprint: 100,
        peak: 200,
        disk_read: 0,
        disk_written: 0,
        at: Instant::now(),
    };
    let mut next = first.clone();
    next.at += Duration::from_secs(2);
    next.footprint = 140;
    assert_eq!(growth(&next, Some(&first)), Some(20));
    next.footprint = 60;
    assert_eq!(growth(&next, Some(&first)), Some(-20));
    assert_eq!(growth(&next, None), None);
    assert_eq!(growth(&first, Some(&first)), None);
    next.pid = 43;
    assert_eq!(growth(&next, Some(&first)), None);
    next.pid = 42;
    next.started = 2;
    assert_eq!(growth(&next, Some(&first)), None);
}

#[cfg(target_os = "macos")]
#[test]
fn os_read_works_for_this_process_and_rejects_invalid_pids() {
    let reading = read(std::process::id()).expect("read own OS memory counters");
    assert!(reading.footprint > 0);
    assert!(reading.resident > 0);
    // XNU's gather_rusage_info reads the lifetime maximum before current
    // footprint, so concurrent allocations may make the latter larger.
    // Verify each OS reading instead of assuming an atomic snapshot.
    // https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_resource.c
    assert!(reading.peak > 0);
    assert!(read(0).is_none());
    assert!(read(u32::MAX).is_none());
}
