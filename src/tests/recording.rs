// SPDX-License-Identifier: MIT
use super::*;
use crate::domain::{RequestUsage, TelemetrySource};
use std::time::{Duration, SystemTime};

fn decoding(second: u64, tps: f64, swap: u64) -> Sample {
    Sample {
        llm_provider: "oMLX".into(),
        llm_model: "Qwen3.8-27B-oQ4e-mtp".into(),
        llm_source: TelemetrySource::Live,
        llm_status: "generating".into(),
        llm_generation_tps: Some(tps),
        llm_generation_tps_live: true,
        llm_active_requests: Some(1),
        llm_observed_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(second)),
        updated: format!("12:00:{second:02}"),
        swap_in: swap,
        rate_ready: true,
        llm_requests: vec![RequestUsage {
            id: "r1".into(),
            prompt: 30_000,
            output: Some(second * 50),
            ..RequestUsage::default()
        }],
        ..Sample::default()
    }
}

#[test]
fn a_recording_round_trips_and_replays_a_slowdown_with_its_reason() {
    let thresholds = Thresholds::from_config(&Config::default());
    let swap = thresholds.swap_warn_rate * 2;
    let mut text = Vec::new();
    for second in 0..12 {
        writeln!(
            text,
            "{}",
            serde_json::to_string(&decoding(second, 50.0, 0)).unwrap()
        )
        .unwrap();
    }
    for second in 12..18 {
        writeln!(
            text,
            "{}",
            serde_json::to_string(&decoding(second, 20.0, swap)).unwrap()
        )
        .unwrap();
    }
    for second in 18..24 {
        writeln!(
            text,
            "{}",
            serde_json::to_string(&decoding(second, 50.0, 0)).unwrap()
        )
        .unwrap();
    }
    // A line that is not a sample is skipped, not fatal.
    text.extend_from_slice(b"not json\n");
    let mut samples = read(text.as_slice());
    assert_eq!(samples.len(), 24);
    assert_eq!(samples[3].llm_requests[0].id, "r1");

    let result = replay(&mut samples, thresholds);
    assert_eq!(result.samples, 24);
    assert_eq!(result.generating, 24);
    assert!(result.slow_generation >= 3, "{result:?}");
    assert_eq!(result.slow_prefill, 0);
    let states: Vec<&str> = result.findings.iter().map(|(_, s, _)| s.as_str()).collect();
    assert!(states.contains(&"SLOWDOWN"), "{states:?}");
    assert!(states.contains(&"RECOVERED"), "{states:?}");
    let slowdown = result
        .findings
        .iter()
        .find(|(_, state, _)| state == "SLOWDOWN")
        .unwrap();
    assert!(slowdown.2.contains("paging"), "{}", slowdown.2);
    assert!(slowdown.0.starts_with("12:00:"), "{}", slowdown.0);

    let mut out = Vec::new();
    write_report(&mut out, &result).unwrap();
    let report = String::from_utf8(out).unwrap();
    for line in [
        "SAMPLES     24",
        "GENERATION  24 samples",
        "FINDINGS    1",
        "SLOWDOWN",
    ] {
        assert!(report.contains(line), "missing {line}\n{report}");
    }
    let empty = Replay::default();
    let mut out = Vec::new();
    write_report(&mut out, &empty).unwrap();
    assert!(String::from_utf8(out)
        .unwrap()
        .contains("in a slowdown or stall (—)"));
}

#[test]
fn recording_writes_one_line_per_sample_and_replay_needs_samples() {
    let dir = std::env::temp_dir().join(format!("mlxtop-recording-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session.jsonl");
    let _ = std::fs::remove_file(&path);
    // Writing before recording starts does nothing.
    write(&decoding(0, 50.0, 0));
    start(&path).unwrap();
    assert!(start(&path).is_err(), "one recording per process");
    write(&decoding(1, 50.0, 0));
    write(&decoding(2, 50.0, 0));
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 2);
    let thresholds = Thresholds::from_config(&Config::default());
    let mut out = Vec::new();
    run(&path, thresholds, &mut out).unwrap();
    assert!(String::from_utf8(out).unwrap().contains("SAMPLES     2"));

    let empty = dir.join("empty.jsonl");
    std::fs::write(&empty, "").unwrap();
    let error = run(&empty, thresholds, &mut Vec::new()).unwrap_err();
    assert!(error.to_string().contains("--record"), "{error}");
    assert!(run(&dir.join("missing.jsonl"), thresholds, &mut Vec::new()).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
