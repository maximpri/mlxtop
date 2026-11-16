// SPDX-License-Identifier: MIT
use super::*;
use crate::domain::{CorrelationCause, RequestUsage, TelemetrySource, SAMPLE_SCHEMA};
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
    let (mut samples, _) = read_counted(text.as_slice()).unwrap();
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
    // One reason, told once, for every slow sample; 23 seconds generating.
    assert_eq!(
        result.tally,
        [(
            CorrelationCause::Paging,
            Tally {
                samples: result.slow_generation,
                episodes: 1
            }
        )]
    );
    assert_eq!(result.generating_seconds, 23.0);

    let mut out = Vec::new();
    write_report(&mut out, &result).unwrap();
    let report = String::from_utf8(out).unwrap();
    let paging = format!(
        "paging                    {:>6}         1    156.52",
        result.slow_generation
    );
    for line in [
        "SAMPLES     24",
        "GENERATION  24 samples over 23s · ",
        "REASON                      SLOW  EPISODES  PER HOUR",
        &paging,
        "FINDINGS    1",
        "SLOWDOWN",
    ] {
        assert!(report.contains(line), "missing {line}\n{report}");
    }
    let empty = Replay::default();
    let mut out = Vec::new();
    write_report(&mut out, &empty).unwrap();
    let report = String::from_utf8(out).unwrap();
    assert!(report.contains("GENERATION  0 samples · 0 in a slowdown or stall (—)"));
    assert!(
        !report.contains("REASON"),
        "nothing fired: no tally\n{report}"
    );
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
    // The sink is process-wide and other tests sample too: check that both
    // of these lines are there, not that nothing else is.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("\"12:00:01\"") && text.contains("\"12:00:02\""),
        "{text}"
    );
    assert!(
        !text.contains("\"12:00:00\""),
        "written before recording started"
    );
    let thresholds = Thresholds::from_config(&Config::default());
    let mut out = Vec::new();
    run(&path, thresholds, None, &mut out).unwrap();
    let report = String::from_utf8(out).unwrap();
    let samples: usize = report
        .lines()
        .find_map(|line| line.strip_prefix("SAMPLES"))
        .and_then(|count| count.trim().parse().ok())
        .unwrap();
    assert!(samples >= 2, "{report}");

    let empty = dir.join("empty.jsonl");
    std::fs::write(&empty, "").unwrap();
    let error = run(&empty, thresholds, None, &mut Vec::new()).unwrap_err();
    assert!(error.to_string().contains("--record"), "{error}");
    assert!(run(
        &dir.join("missing.jsonl"),
        thresholds,
        None,
        &mut Vec::new()
    )
    .is_err());
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
        footprint: None,
    }];
    let line = serde_json::to_string(&sample).unwrap();
    assert!(!line.contains("SECRET"), "{line}");
    assert!(!line.contains("\"command\""), "{line}");
    let (back, _) = read_counted(line.as_bytes()).unwrap();
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
fn recordings_made_before_samples_carried_their_time_still_read() {
    let mut sample = decoding(1, 50.0, 0);
    sample.ts = Some(1_794_719_479_668);
    let line = serde_json::to_string(&sample).unwrap();
    assert!(line.contains("\"ts\":1794719479668"), "{line}");
    assert!(
        line.contains(&format!("\"schema\":{SAMPLE_SCHEMA}")),
        "{line}"
    );
    let (back, _) = read_counted(line.as_bytes()).unwrap();
    assert_eq!(back[0].ts, Some(1_794_719_479_668));
    assert_eq!(back[0].schema, SAMPLE_SCHEMA);
    // A line written before either field existed: its time is unknown, not
    // zero, and it says it carries no format number.
    let mut old: serde_json::Value = serde_json::from_str(&line).unwrap();
    let fields = old.as_object_mut().unwrap();
    fields.remove("ts");
    fields.remove("schema");
    let (back, skipped) = read_counted(old.to_string().as_bytes()).unwrap();
    assert_eq!(skipped, 0);
    assert_eq!(back[0].ts, None);
    assert_eq!(back[0].schema, 0);
    assert_eq!(back[0].updated, "12:00:01");
}

#[test]
fn a_file_of_unrelated_json_objects_is_not_a_recording() {
    let (samples, skipped) = read_counted("{\"a\": 1}\n{\"b\": 2}\n".as_bytes()).unwrap();
    assert!(samples.is_empty());
    assert_eq!(skipped, 2);
}

fn polled(at: u64, tps: f64, out: u64, swap: u64) -> Sample {
    let mut sample = decoding(0, tps, swap);
    sample.llm_observed_at = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(at));
    sample.updated = format!("t{at}");
    sample.llm_requests[0].id = "long".into();
    sample.llm_requests[0].output = Some(out);
    sample
}

#[test]
fn a_slowdown_is_found_at_every_supported_interval() {
    let thresholds = Thresholds::from_config(&Config::default());
    for step in [1u64, 10, 30, 60] {
        let mut samples = Vec::new();
        for i in 0..12u64 {
            samples.push(polled(i * step, 50.0, 100 + i * 50, 0));
        }
        for i in 12..24u64 {
            samples.push(polled(
                i * step,
                20.0,
                700 + i * 20,
                thresholds.swap_warn_rate * 2,
            ));
        }
        let result = replay(&mut samples, thresholds);
        assert!(
            result
                .findings
                .iter()
                .any(|(_, state, _)| state == "SLOWDOWN"),
            "interval {step}s: {:?}",
            result.findings
        );
    }
}

#[test]
fn the_session_counts_work_at_a_sixty_second_interval() {
    let mut session = crate::session::Session::default();
    for i in 0..20u64 {
        session.observe(&polled(i * 60, 50.0, 100, 0));
    }
    assert!(session.generating >= 60.0 * 18.0, "{}", session.generating);
    // A pause at a one-second interval is still not counted as work.
    let mut session = crate::session::Session::default();
    for at in [0u64, 1, 2, 3, 120, 121] {
        session.observe(&polled(at, 50.0, 100, 0));
    }
    assert!(session.generating <= 4.0, "{}", session.generating);
}

#[test]
fn rate_windows_follow_the_poll_interval() {
    let five = Duration::from_secs(5);
    assert_eq!(crate::providers::rate_span_for(five, 1), five);
    assert_eq!(
        crate::providers::rate_span_for(five, 10),
        Duration::from_secs(22)
    );
    assert_eq!(
        crate::providers::rate_span_for(Duration::from_secs(30), 60),
        Duration::from_secs(122)
    );
}

#[test]
fn a_prompt_read_on_a_runtime_without_a_request_list_is_not_a_slowdown() {
    let thresholds = Thresholds::from_config(&Config::default());
    let base = |at: u64, tps: f64| Sample {
        llm_provider: "vLLM".into(),
        llm_model: "m".into(),
        llm_source: TelemetrySource::Live,
        llm_status: "generating".into(),
        llm_generation_tps: Some(tps),
        llm_generation_tps_live: true,
        llm_prefill_tps: Some(0.0),
        llm_prefill_tps_live: true,
        llm_active_requests: Some(1),
        llm_observed_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(at)),
        updated: format!("t{at}"),
        rate_ready: true,
        ..Sample::default()
    };
    let mut samples: Vec<Sample> = (0..15).map(|i| base(i, 50.0)).collect();
    samples.extend((15..25).map(|i| base(i, 0.0)));
    samples.extend((25..35).map(|i| base(i, 50.0)));
    let result = replay(&mut samples, thresholds);
    assert!(result.findings.is_empty(), "{:?}", result.findings);
    assert_eq!(result.slow_generation, 0);
}

#[test]
fn a_slowdown_that_ends_with_its_request_is_closed_in_the_journal() {
    let thresholds = Thresholds::from_config(&Config::default());
    let mut samples = Vec::new();
    for i in 0..12u64 {
        samples.push(polled(i, 50.0, 100 + i * 50, 0));
    }
    for i in 12..18u64 {
        samples.push(polled(i, 20.0, 700 + i * 20, thresholds.swap_warn_rate * 2));
    }
    for i in 18..138u64 {
        let mut idle = polled(i, 0.0, 0, 0);
        idle.llm_requests.clear();
        idle.llm_generation_tps = None;
        idle.llm_generation_tps_live = false;
        idle.llm_active_requests = Some(0);
        idle.llm_status = "idle".into();
        samples.push(idle);
    }
    for i in 138..150u64 {
        let mut next = polled(i, 50.0, 100 + (i - 138) * 50, 0);
        next.llm_requests[0].id = "b".into();
        samples.push(next);
    }
    let result = replay(&mut samples, thresholds);
    let states: Vec<&str> = result.findings.iter().map(|(_, s, _)| s.as_str()).collect();
    assert_eq!(states.first(), Some(&"SLOWDOWN"), "{states:?}");
    assert!(
        states
            .iter()
            .any(|s| matches!(*s, "ENDED" | "RECOVERED" | "SETTLED")),
        "{:?}",
        result.findings
    );
}

/// A session that slows for one reason after another: paging, a stall,
/// paging again within a minute (the same story, told once), compression,
/// nothing at all, and then a prompt read slowly while the machine pages.
fn eventful(thresholds: Thresholds) -> Vec<Sample> {
    let swap = thresholds.swap_warn_rate * 2;
    let churn = thresholds.compression_warn_rate * 4;
    // Speed, samples, swap and compression, and whether it stalled.
    let plan = [
        (50.0, 12, 0, 0, false),
        (20.0, 6, swap, 0, false),
        (50.0, 12, 0, 0, false),
        (0.0, 4, 0, 0, true),
        (50.0, 14, 0, 0, false),
        (20.0, 6, swap, 0, false),
        (50.0, 14, 0, 0, false),
        (20.0, 6, 0, churn, false),
        (50.0, 14, 0, 0, false),
        (20.0, 6, 0, 0, false),
        (50.0, 14, 0, 0, false),
    ];
    let mut samples = Vec::new();
    let (mut at, mut out) = (0u64, 0u64);
    for (speed, count, swap, churn, stalled) in plan {
        for _ in 0..count {
            out += speed as u64;
            let mut sample = polled(at, speed, out, swap);
            sample.compress = churn;
            if stalled {
                sample.llm_requests[0].idle_for = Some(Duration::from_secs(3));
            }
            samples.push(sample);
            at += 1;
        }
    }
    for step in 0..18u64 {
        let slow = step >= 12;
        let mut sample = polled(at, 0.0, out, if slow { swap } else { 0 });
        sample.llm_generation_tps = None;
        sample.llm_generation_tps_live = false;
        sample.llm_prefill_tps = Some(if slow { 80.0 } else { 300.0 });
        sample.llm_prefill_tps_live = true;
        sample.llm_requests[0].id = "prompt".into();
        sample.llm_requests[0].prefilled = Some(step * 300);
        samples.push(sample);
        at += 1;
    }
    for sample in &mut samples {
        sample.ts = sample
            .llm_observed_at
            .and_then(crate::collector::epoch_millis);
    }
    samples
}

#[test]
fn the_tally_counts_the_rows_the_live_journal_writes() {
    let thresholds = Thresholds::from_config(&Config::default());
    // The live path: each sample as the collector concludes one it read.
    let mut live = Collector::with_host(
        3600,
        Config::default(),
        Box::new(Nothing),
        Platform::MacOs,
        None,
    );
    let mut rows = Vec::new();
    let mut slow = Vec::new();
    let mut recording = Vec::new();
    for sample in eventful(thresholds) {
        let taken = live.conclude(sample, std::time::Instant::now());
        for row in live.signals.drain(..) {
            if matches!(row.state.as_str(), "SLOWDOWN" | "STALL") {
                rows.push(taken.correlation.cause);
            }
        }
        if taken.correlation.is_material_drop() {
            slow.push(taken.correlation.cause);
        }
        writeln!(recording, "{}", serde_json::to_string(&taken).unwrap()).unwrap();
    }
    use CorrelationCause::{Compression, None as Unexplained, Paging, Stall};
    assert_eq!(rows, [Paging, Stall, Compression, Unexplained, Paging]);

    // The same session read back from its recording.
    let (mut samples, skipped) = read_counted(recording.as_slice()).unwrap();
    assert_eq!(skipped, 0);
    let result = replay(&mut samples, thresholds);
    let given = |cause: CorrelationCause| {
        result
            .tally
            .iter()
            .find(|(reason, _)| *reason == cause)
            .map(|(_, tally)| *tally)
            .unwrap_or_default()
    };
    for cause in [Paging, Stall, Compression, Unexplained] {
        let told = rows.iter().filter(|row| **row == cause).count();
        let marked = slow.iter().filter(|sample| **sample == cause).count();
        assert_eq!(
            given(cause),
            Tally {
                samples: marked,
                episodes: told
            },
            "{cause:?}"
        );
    }
    assert_eq!(result.tally.len(), 4, "{:?}", result.tally);
    // Paging again within a minute is the same story: its samples count,
    // its row is not written again.
    assert_eq!(given(Paging).episodes, 2);
    assert!(given(Paging).samples > given(Compression).samples);
    // Episodes are counted per hour of generation, stalls included.
    assert_eq!(result.generating_seconds, live.session.generating);
    assert!(result.generating_seconds > 100.0);

    // A recording made before samples carried their time replays the same.
    let old: Vec<u8> = recording
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .flat_map(|line| {
            let mut value: serde_json::Value = serde_json::from_slice(line).unwrap();
            let fields = value.as_object_mut().unwrap();
            assert!(fields.remove("ts").is_some_and(|ts| ts.is_u64()));
            fields.remove("schema");
            format!("{value}\n").into_bytes()
        })
        .collect();
    let (mut samples, _) = read_counted(old.as_slice()).unwrap();
    assert!(samples.iter().all(|sample| sample.ts.is_none()));
    assert_eq!(replay(&mut samples, thresholds), result);
}

#[test]
fn the_tally_puts_the_most_told_reason_first_and_counts_it_per_hour() {
    let tally = |samples, episodes| Tally { samples, episodes };
    let mut result = Replay {
        samples: 9_000,
        generating: 7_000,
        slow_generation: 21,
        generating_seconds: 2.0 * 3600.0,
        tally: vec![
            (CorrelationCause::None, tally(6, 1)),
            (CorrelationCause::Stall, tally(3, 1)),
            (CorrelationCause::Paging, tally(12, 3)),
        ],
        ..Replay::default()
    };
    let mut out = Vec::new();
    write_report(&mut out, &result).unwrap();
    let report = String::from_utf8(out).unwrap();
    let tally_lines: Vec<&str> = report
        .lines()
        .skip_while(|line| !line.starts_with("REASON"))
        .take_while(|line| !line.starts_with("FINDINGS"))
        .collect();
    assert_eq!(
        tally_lines,
        [
            "REASON                      SLOW  EPISODES  PER HOUR",
            "paging                        12         3      1.50",
            "nothing else unusual           6         1      0.50",
            "generation stalled             3         1      0.50",
        ],
        "{report}"
    );
    assert!(report.contains("GENERATION  7000 samples over 2h 0m · 21 in a slowdown"));
    // With no time measured there is no rate: unknown, not zero.
    result.generating_seconds = 0.0;
    let mut out = Vec::new();
    write_report(&mut out, &result).unwrap();
    let report = String::from_utf8(out).unwrap();
    assert!(report.contains("GENERATION  7000 samples · 21"), "{report}");
    assert!(
        report.contains("paging                        12         3         —"),
        "{report}"
    );
}

const MODEL: &str = "Qwen3.8-27B-oQ4e-mtp";

/// A request oMLX lists: reading its prompt, or writing at `tps`.
fn listed(id: &str, prompt: u64, tps: Option<f64>) -> RequestUsage {
    RequestUsage {
        provider: "oMLX".into(),
        model: MODEL.into(),
        id: id.into(),
        prompt,
        output: tps.map(|_| 400),
        output_tps: tps,
        prefilled: tps.is_none().then_some(prompt / 2),
        ..RequestUsage::default()
    }
}

/// A session in which first tokens lose time four ways, a poll a second,
/// with oMLX set to run one request at a time: ten pairs of requests at
/// once at 58 tok/s each against 120 alone, the second of each waiting 6 s
/// for its first token against 2 s alone; a 45k-token prompt read in full
/// at 255 tok/s right after a 44k one 98% cached; a 6.5 s model load the
/// usage recorder reported; three requests recomputed.
fn first_tokens_lost() -> Vec<Sample> {
    let mut samples = Vec::new();
    let mut poll = |requests: Vec<RequestUsage>, change: &dyn Fn(&mut Sample)| {
        let second = samples.len() as u64;
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + second);
        let mut sample = Sample {
            llm_provider: "oMLX".into(),
            llm_model: MODEL.into(),
            llm_source: TelemetrySource::Live,
            llm_status: "generating".into(),
            llm_observed_at: Some(at),
            updated: format!("14:{:02}:{:02}", second / 60 % 60, second % 60),
            llm_requests: requests
                .into_iter()
                .map(|request| RequestUsage {
                    observed_at: Some(at),
                    ..request
                })
                .collect(),
            llm_max_concurrent: Some(1),
            llm_preemptions_total: Some(0),
            ..Sample::default()
        };
        change(&mut sample);
        sample.ts = crate::collector::epoch_millis(at);
        samples.push(sample);
    };
    let same = |_: &mut Sample| {};
    for n in 0..12 {
        let id = format!("alone-{n}");
        for _ in 0..2 {
            poll(vec![listed(&id, 900, None)], &same);
        }
        for _ in 0..4 {
            poll(vec![listed(&id, 900, Some(120.0))], &same);
        }
    }
    for n in 0..10 {
        let (a, b) = (format!("a-{n}"), format!("b-{n}"));
        for _ in 0..2 {
            poll(vec![listed(&a, 900, None)], &same);
        }
        for _ in 0..3 {
            poll(vec![listed(&a, 900, Some(120.0))], &same);
        }
        for _ in 0..6 {
            poll(
                vec![listed(&a, 900, Some(120.0)), listed(&b, 900, None)],
                &same,
            );
        }
        for _ in 0..20 {
            poll(
                vec![listed(&a, 900, Some(58.0)), listed(&b, 900, Some(58.0))],
                &same,
            );
        }
        for _ in 0..3 {
            poll(vec![listed(&b, 900, Some(120.0))], &same);
        }
    }
    // oMLX adds each request's prompt and cached tokens to its totals as it
    // ends; prompts are read at 255 tok/s.
    for (requests, prompt, cached) in [
        (vec![listed("a", 44_000, Some(60.0))], 0, 0),
        (vec![listed("a", 44_000, Some(60.0))], 0, 0),
        (Vec::new(), 44_000, 43_200),
        (vec![listed("b", 45_000, None)], 44_000, 43_200),
        (vec![listed("b", 45_000, Some(60.0))], 44_000, 43_200),
        (Vec::new(), 89_000, 43_200),
    ] {
        poll(requests, &|sample: &mut Sample| {
            sample.llm_total_prompt_tokens = Some(prompt);
            sample.llm_total_cached_tokens = Some(cached);
            sample.llm_prefill_tps = Some(255.0);
            sample.llm_prefill_tps_live = true;
        });
    }
    // A usage recorder's record of a request that waited for its model.
    let cold = RequestUsage {
        completed: true,
        ..listed("cold", 1_200, Some(40.0))
    };
    for _ in 0..2 {
        poll(vec![cold.clone()], &|sample: &mut Sample| {
            sample.llm_request_timings = vec![crate::domain::RequestTiming {
                provider: "oMLX".into(),
                model: MODEL.into(),
                id: "cold".into(),
                prefill_tps: None,
                load_ms: Some(6_500),
            }];
        });
    }
    poll(Vec::new(), &|sample: &mut Sample| {
        sample.llm_preemptions_total = Some(3);
    });
    samples
}

#[test]
fn replay_counts_what_first_tokens_lost_as_the_live_journal_tells_it() {
    let thresholds = Thresholds::from_config(&Config::default());
    // The live path: each sample as the collector concludes one it read.
    let mut live = Collector::with_host(
        3600,
        Config::default(),
        Box::new(Nothing),
        Platform::MacOs,
        None,
    );
    let mut rows = Vec::new();
    let mut recording = Vec::new();
    for sample in first_tokens_lost() {
        let taken = live.conclude(sample, std::time::Instant::now());
        for row in live.signals.drain(..) {
            if before_first_token(&row.state).is_some() {
                rows.push((row.time, row.state, row.summary));
            }
        }
        writeln!(recording, "{}", serde_json::to_string(&taken).unwrap()).unwrap();
    }
    let states: Vec<&str> = rows.iter().map(|(_, state, _)| state.as_str()).collect();
    assert_eq!(
        states,
        ["OVERLAP", "CACHE MISS", "MODEL LOAD", "KV CACHE FULL"]
    );

    // The same session read back from its recording: the same rows, at
    // the same times, among the findings.
    let (mut samples, skipped) = read_counted(recording.as_slice()).unwrap();
    assert_eq!(skipped, 0);
    let result = replay(&mut samples, thresholds);
    let told: Vec<(String, String, String)> = result
        .findings
        .iter()
        .filter(|(_, state, _)| before_first_token(state).is_some())
        .cloned()
        .collect();
    assert_eq!(told, rows);
    // Each counted, with what it cost first tokens as the session summary
    // counts it: 45,000 tokens at 255 tok/s, the load, and 4 s beyond the
    // usual 2 s wait for each of the ten that started beside another.
    let tally = |state, name, rows, lost| Told {
        state,
        name,
        rows,
        lost,
    };
    assert_eq!(
        result.before_first_token,
        [
            tally("OVERLAP", "overlap", 1, Some(40.0)),
            tally("CACHE MISS", "cache miss", 1, Some(45_000.0 / 255.0)),
            tally("MODEL LOAD", "model load", 1, Some(6.5)),
            tally("KV CACHE FULL", "KV cache full", 1, None),
        ]
    );
    let summary: Vec<f64> = live
        .session
        .request_findings
        .lost()
        .iter()
        .map(|(_, seconds)| *seconds)
        .collect();
    assert_eq!(summary, [45_000.0 / 255.0, 6.5, 40.0]);
    assert!(result.tally.is_empty(), "{:?}", result.tally);

    let mut out = Vec::new();
    write_report(&mut out, &result).unwrap();
    let report = String::from_utf8(out).unwrap();
    for line in [
        "BEFORE FIRST TOKEN          ROWS  PER HOUR      LOST",
        "overlap                        1         —       40s",
        "cache miss                     1         —    2m 56s",
        "model load                     1         —        7s",
        "KV cache full                  1         —         —",
        "FINDINGS    4",
        "14:06:29  OVERLAP       two requests at once gained no throughput (0.97× one at a time) and made first tokens wait 3.0× as long · oMLX is set to run 1 at a time",
        "14:06:57  CACHE MISS    b · 45.0k prompt read in full right after one of 44.0k was 98% cached · ≈2m 56s of prompt reading",
        "14:06:58  MODEL LOAD    cold · first token waited 6.5s for the model to load",
        "14:07:00  KV CACHE FULL 3 requests were recomputed",
    ] {
        assert!(report.contains(line), "missing {line}\n{report}");
    }
    assert!(!report.contains("REASON"), "{report}");
}

#[test]
fn findings_before_first_tokens_are_tallied_the_most_told_first() {
    let told = |state, name, rows, lost| Told {
        state,
        name,
        rows,
        lost,
    };
    let result = Replay {
        samples: 9_000,
        generating: 7_000,
        generating_seconds: 2.0 * 3600.0,
        before_first_token: vec![
            told("MODEL LOAD", "model load", 2, Some(13.0)),
            told("KV CACHE FULL", "KV cache full", 4, None),
            told("CACHE MISS", "cache miss", 5, Some(564.0)),
            told("OVERLAP", "overlap", 2, Some(0.4)),
        ],
        findings: vec![
            ("09:00:00".into(), "SLOWDOWN".into(), "GEN ↓31.0%".into()),
            ("09:00:30".into(), "RECOVERED".into(), "GEN back".into()),
        ],
        ..Replay::default()
    };
    let mut out = Vec::new();
    write_report(&mut out, &result).unwrap();
    let report = String::from_utf8(out).unwrap();
    let tally: Vec<&str> = report
        .lines()
        .skip_while(|line| !line.starts_with("BEFORE FIRST TOKEN"))
        .take_while(|line| !line.starts_with("FINDINGS"))
        .collect();
    // Equal counts keep the order first told; a cost under a second is
    // still a cost; a full KV cache's is not counted.
    assert_eq!(
        tally,
        [
            "BEFORE FIRST TOKEN          ROWS  PER HOUR      LOST",
            "cache miss                     5      2.50    9m 24s",
            "KV cache full                  4      2.00         —",
            "model load                     2      1.00       13s",
            "overlap                        2      1.00        0s",
        ],
        "{report}"
    );
    // The findings told are the slowdown alone here; with no finding before
    // first tokens among them, the rows read as they always have.
    assert!(report.contains("FINDINGS    1\n"), "{report}");
    assert!(
        report.ends_with("09:00:00  SLOWDOWN  GEN ↓31.0%\n09:00:30  RECOVERED GEN back\n"),
        "{report}"
    );
}

#[test]
fn every_row_a_finding_writes_is_counted_once() {
    let thresholds = Thresholds::from_config(&Config::default());
    // A runtime recomputing requests for want of KV cache: 3, then 1 more
    // within a minute (the same story, no row), then 3 more two minutes on.
    let mut samples: Vec<Sample> = (0..130u64)
        .map(|second| Sample {
            llm_provider: "vLLM".into(),
            llm_model: "m".into(),
            llm_source: TelemetrySource::Live,
            llm_status: "idle".into(),
            llm_observed_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(second)),
            updated: format!("t{second}"),
            llm_preemptions_total: Some(match second {
                0..10 => 5,
                10..20 => 8,
                20..120 => 9,
                _ => 12,
            }),
            ..Sample::default()
        })
        .collect();
    let result = replay(&mut samples, thresholds);
    assert_eq!(
        result.before_first_token,
        [Told {
            state: "KV CACHE FULL",
            name: "KV cache full",
            rows: 2,
            lost: None,
        }]
    );
    let rows: Vec<(&str, &str)> = result
        .findings
        .iter()
        .map(|(time, _, summary)| (time.as_str(), summary.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("t10", "3 requests were recomputed"),
            ("t120", "3 requests were recomputed"),
        ]
    );
    let mut out = Vec::new();
    write_report(&mut out, &result).unwrap();
    let report = String::from_utf8(out).unwrap();
    assert!(
        report.contains("KV cache full                  2         —         —\nFINDINGS    2\n"),
        "{report}"
    );
}
