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

#[test]
fn recordings_hold_no_process_command_lines() {
    let mut sample = decoding(1, 50.0, 0);
    sample.llm_processes = vec![crate::domain::LlmProcess {
        pid: 42,
        name: "server".into(),
        command: "server --api-key SECRET".into(),
        rss: 1,
        cpu: 0.0,
        memory_percent: None,
        state: "R".into(),
        pageins: None,
        pagein_rate: None,
    }];
    let line = serde_json::to_string(&sample).unwrap();
    assert!(!line.contains("SECRET"), "{line}");
    assert!(!line.contains("\"command\""), "{line}");
    let back = read(line.as_bytes());
    assert_eq!(back.len(), 1);
    assert!(back[0].llm_processes.is_empty());
    assert_eq!(back[0].llm_generation_tps, Some(50.0));
}

#[cfg(unix)]
#[test]
fn a_new_recording_is_readable_by_its_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("mlxtop-rec-mode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mode.jsonl");
    let _ = std::fs::remove_file(&path);
    drop(open_private(&path).unwrap());
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(mode, 0o600);
}

#[test]
fn replay_reports_findings_after_hundreds_of_requests() {
    let thresholds = Thresholds::from_config(&Config::default());
    let swap = thresholds.swap_warn_rate * 2;
    let mut samples = Vec::new();
    // 130 short requests: a PROMPT and a DONE row each fill the Journal.
    for request in 0..130u64 {
        let mut busy = decoding(request * 3, 50.0, 0);
        busy.llm_requests[0].id = format!("r{request}");
        samples.push(busy);
        let mut idle = decoding(request * 3 + 1, 50.0, 0);
        idle.llm_requests[0].id = format!("r{request}");
        idle.llm_requests[0].completed = true;
        idle.llm_status = "idle".into();
        idle.llm_active_requests = Some(0);
        idle.llm_generation_tps_live = false;
        samples.push(idle);
    }
    let base = 130 * 3 + 10;
    for second in 0..12 {
        samples.push(decoding(base + second, 50.0, 0));
    }
    for second in 12..24 {
        samples.push(decoding(base + second, 20.0, swap));
    }
    for sample in &mut samples {
        sample.updated = format!(
            "{}",
            sample
                .llm_observed_at
                .unwrap()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        );
    }
    let result = replay(&mut samples, thresholds);
    assert!(
        result
            .findings
            .iter()
            .any(|(_, state, _)| state == "SLOWDOWN"),
        "{:?}",
        result.findings
    );
}

#[test]
fn replay_reports_lines_it_could_not_read() {
    let good = serde_json::to_string(&decoding(1, 50.0, 0)).unwrap();
    let text = format!("{good}\nnot json\n{{\"other\": 1}}\n\n{good}\n");
    let (samples, skipped) = read_counted(text.as_bytes()).unwrap();
    assert_eq!(
        samples.len(),
        2,
        "the bad line in the middle must not stop reading"
    );
    assert_eq!(skipped, 2);
    let report = Replay {
        samples: 2,
        skipped,
        ..Replay::default()
    };
    let mut out = Vec::new();
    write_report(&mut out, &report).unwrap();
    assert!(String::from_utf8(out)
        .unwrap()
        .contains("SKIPPED     2 lines"));
}

#[test]
fn a_file_of_unrelated_json_objects_is_not_a_recording() {
    let (samples, skipped) = read_counted("{\"a\": 1}\n{\"b\": 2}\n".as_bytes()).unwrap();
    assert!(samples.is_empty());
    assert_eq!(skipped, 2);
}
