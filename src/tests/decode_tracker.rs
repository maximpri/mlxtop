// SPDX-License-Identifier: MIT
use super::*;
use std::time::{Duration, SystemTime};

fn generating(id: &str, output: u64, average: f64) -> RequestUsage {
    RequestUsage {
        provider: "oMLX".into(),
        model: "m".into(),
        id: id.into(),
        prompt: 67_793,
        cached: None,
        output: Some(output),
        output_tps: Some(average),
        completed: false,
        queued: false,
        prefilled: None,
        idle_for: None,
        ttft_ms: None,
        observed_at: Some(SystemTime::now()),
    }
}

#[test]
fn a_stall_reads_zero_where_the_runtime_average_barely_moves() {
    // Captured from oMLX: 1,693 tokens at 37.6 s, none for the next four
    // polls, while `tokens_per_second` (the average since generation began)
    // only slid from 45.1 to 40.7.
    let mut tracker = DecodeTracker::default();
    let start = Instant::now();
    let polls = [
        (1665, 45.6),
        (1693, 45.1),
        (1693, 43.9),
        (1693, 42.8),
        (1693, 41.7),
        (1693, 40.7),
        (1724, 40.5),
    ];
    let speeds: Vec<f64> = polls
        .iter()
        .enumerate()
        .map(|(second, (tokens, average))| {
            let mut requests = [generating("e28cde", *tokens, *average)];
            tracker
                .apply(&mut requests, start + Duration::from_secs(second as u64))
                .unwrap()
        })
        .collect();
    // The first poll has nothing to compare with and keeps the runtime's figure.
    assert_eq!(speeds[0], 45.6);
    assert_eq!(speeds[1], 28.0);
    assert_eq!(&speeds[2..6], &[0.0; 4]);
    assert_eq!(speeds[6], 31.0);
}

#[test]
fn a_prefill_starts_from_its_cached_prefix() {
    // Captured from oMLX: the next request's prefill reports 67,776 of
    // 68,848 processed from its first poll, the previous prompt's prefix.
    let mut tracker = DecodeTracker::default();
    let mut prefill = generating("next", 0, 0.0);
    prefill.output = None;
    prefill.output_tps = None;
    prefill.prompt = 68_848;
    prefill.prefilled = Some(67_776);
    let mut requests = [prefill.clone()];
    assert_eq!(tracker.apply(&mut requests, Instant::now()), None);
    assert_eq!(requests[0].cached, Some(67_776));
    // Later polls keep the prefix from the first one, not the progress.
    prefill.prefilled = Some(68_000);
    let mut requests = [prefill];
    tracker.apply(&mut requests, Instant::now());
    assert_eq!(requests[0].cached, Some(67_776));
}

#[test]
fn the_runtime_reported_cache_share_is_kept() {
    let mut tracker = DecodeTracker::default();
    let mut prefill = generating("r", 0, 0.0);
    prefill.output = None;
    prefill.prefilled = Some(60_000);
    prefill.cached = Some(50_000);
    let mut requests = [prefill];
    tracker.apply(&mut requests, Instant::now());
    assert_eq!(requests[0].cached, Some(50_000));
}
