// SPDX-License-Identifier: MIT
//! Decode per verify pass, checked against the owner's Mac Studio: an M4 Max
//! with the 32-core GPU (410 GB/s) serving Qwen3.8-27B through Splash with a
//! DFlash2 draft, whose request log the research read band by band
//! (`server_ground_truth.md`, questions 3 and 4).
use super::*;
use crate::config::Thresholds;
use crate::domain::{
    CorrelationCause, CorrelationInsight, DropEpisode, RequestUsage, TelemetrySource,
    ThroughputDirection, MIB,
};
use crate::host::Platform;
use crate::model_facts::ModelFacts;

const GIB: u64 = 1024 * MIB;

/// What the server's model reads and holds, as its model facts give it:
/// 15,020,479,488 bytes a pass (decoder layers and output head), 33,280 KV
/// bytes a token (16 layers keep int8 KV, a byte of scale per 64 values),
/// 152.6 MiB of state a sequence, and a draft.
fn server_facts() -> ModelFacts {
    ModelFacts {
        bytes_per_pass: Some(15_020_479_488),
        kv_layers: Some(16),
        kv_heads: Some(4),
        head_dim: Some(256),
        kv_bytes_per_element: Some(1.0 + 1.0 / 64.0),
        fixed_state_bytes: Some(160_038_912),
        draft: Some(true),
        ..ModelFacts::default()
    }
}

/// The server writing one answer at `speed`, `context` tokens deep, its
/// runtime counting `tokens_per_pass`, while generation keeps its usual
/// speed.
fn server(speed: f64, tokens_per_pass: Option<f64>, context: u64) -> Sample {
    let mut sample = Sample {
        total_memory: 36 * GIB,
        impact: "LLM READY".into(),
        rate_ready: true,
        pressure: "GREEN".into(),
        llm_provider: "oMLX".into(),
        llm_model: "Qwen3.8-27B-oQ4e-mtp".into(),
        llm_status: "generating".into(),
        llm_source: TelemetrySource::Live,
        llm_generation_tps: Some(speed),
        llm_generation_tps_live: true,
        llm_tokens_per_pass: tokens_per_pass,
        llm_active_requests: Some(1),
        llm_requests: vec![RequestUsage {
            id: "req-1".into(),
            prompt: context - context / 10,
            output: Some(context / 10),
            ..RequestUsage::default()
        }],
        model: server_facts(),
        correlation: CorrelationInsight {
            direction: ThroughputDirection::Flat,
            ..CorrelationInsight::default()
        },
        ..Sample::default()
    };
    sample.metal.device_name = Some("Apple M4 Max".into());
    sample.metal.gpu_cores = Some(32);
    sample
}

/// A short request with the draft accepted almost every time: the median
/// of the server's requests under 4k (122.8 tok/s at 7.8 tokens a pass).
fn short() -> Sample {
    server(122.8, Some(7.8), 1_500)
}

/// The same model decoding without a draft, at one token a pass.
fn without_draft(speed: f64) -> Sample {
    let mut sample = server(speed, None, 1_500);
    sample.model.draft = Some(false);
    sample
}

/// A record that has seen `sample` `times` times.
fn seen(sample: &Sample, times: usize) -> Record {
    let mut record = Record::default();
    for _ in 0..times {
        record.observe(sample);
    }
    record
}

/// The server's single requests by context (Q4): each band's median tok/s
/// and verify passes a second, a context typical of the band, and the share
/// of 410 GB/s the research worked out from weights and KV.
const BANDS_OF_THE_SERVER: [(&str, u64, f64, f64, f64); 7] = [
    ("under 2k", 1_200, 124.7, 16.14, 59.0),
    ("2–4k", 3_000, 92.7, 15.97, 59.0),
    ("4–8k", 6_000, 72.4, 15.81, 59.0),
    ("8–16k", 12_000, 65.7, 15.36, 58.0),
    ("16–32k", 24_000, 62.1, 14.31, 56.0),
    ("32–64k", 48_000, 55.7, 12.96, 53.0),
    ("64–128k", 80_000, 51.5, 11.59, 50.0),
];

#[test]
fn the_owners_server_reads_59_percent_under_2k_falling_to_50_at_64k_to_128k() {
    assert_eq!(server_facts().kv_bytes_per_token(), Some(33_280));
    for (band, context, speed, passes, research) in BANDS_OF_THE_SERVER {
        let estimate = of_sample(&server(speed, Some(speed / passes), context)).unwrap();
        let share = estimate.share * 100.0;
        assert!(
            (share - research).abs() <= 3.0,
            "{band}: {share:.1}% against {research}%"
        );
        assert!((estimate.passes - passes).abs() < 1e-9, "{band}");
        assert_eq!(
            estimate.bytes_per_pass,
            15_020_479_488 + 160_038_912 + context * 33_280
        );
        assert!(estimate.draft);
    }
    // The prompt bands' medians (Q3) agree: 122.8 tok/s at 7.8 tokens a
    // pass under 4k, 51.6 at 4.5 from 64k.
    let under_4k = of_sample(&short()).unwrap();
    assert!((under_4k.share * 100.0 - 59.0).abs() <= 3.0);
    let from_64k = of_sample(&server(51.6, Some(4.5), 80_000)).unwrap();
    assert!((from_64k.share * 100.0 - 50.0).abs() <= 3.0);
    // The ceiling without a draft: about 27 tok/s short, 23 at 80k.
    assert!((under_4k.ceiling() - 26.92).abs() < 0.01);
    assert!((from_64k.ceiling() - 22.98).abs() < 0.01);
}

#[test]
fn a_share_above_the_peak_is_unknown_never_shown() {
    // Counted per token, the short request reads 456% of 410 GB/s.
    let per_token = server(122.8, Some(1.0), 1_200);
    assert_eq!(of_sample(&per_token), None);
    let mut text = Vec::new();
    crate::report::write_static(
        &mut text,
        &per_token,
        1,
        Thresholds::default(),
        Platform::MacOs,
    )
    .unwrap();
    let text = String::from_utf8(text).unwrap();
    assert!(!text.contains("DECODE"), "{text}");
    let lines = crate::report::diagnostic_lines(&per_token, Some(&seen(&per_token, 30)));
    assert!(
        !lines.iter().any(|line| line.contains("Decode")),
        "{lines:?}"
    );
    // Nothing above the peak is kept either, so no best can come of it.
    assert!(seen(&per_token, 30).summary().is_empty());
    // Just under the peak is still a reading.
    let at_peak = 410e9 / (15_020_479_488.0 + 160_038_912.0 + 1_200.0 * 33_280.0);
    assert!(of_sample(&server(at_peak * 0.999, Some(1.0), 1_200)).is_some());
}

#[test]
fn a_draft_that_does_not_report_tokens_per_pass_shows_nothing() {
    let quiet = server(122.8, None, 1_500);
    assert_eq!(of_sample(&quiet), None);
    let record = seen(&quiet, 30);
    assert!(record.summary().is_empty());
    let lines = crate::report::diagnostic_lines(&quiet, Some(&record));
    assert!(
        !lines.iter().any(|line| line.contains("Decode")),
        "{lines:?}"
    );
    let finding = crate::diagnosis::assess_session(&quiet, &record);
    assert_eq!(finding.next, "");
    // Draft acceptance says a draft runs, whatever the model facts know.
    let mut accepting = quiet.clone();
    accepting.model.draft = None;
    accepting.llm_draft_acceptance = Some(0.9);
    assert_eq!(of_sample(&accepting), None);
    // A draft not known either way is not taken for none.
    let mut unknown = quiet;
    unknown.model.draft = None;
    assert_eq!(of_sample(&unknown), None);
}

#[test]
fn a_model_without_a_draft_emits_one_token_a_pass() {
    let estimate = of_sample(&without_draft(16.14)).unwrap();
    assert_eq!(estimate.tokens_per_pass, 1.0);
    assert!(!estimate.draft);
    assert_eq!(estimate.passes, 16.14);
    assert!((estimate.share * 100.0 - 59.95).abs() < 0.1);
    // Without a draft the share is per token, and there is no payoff to give.
    assert_eq!(
        share_line(&estimate, None),
        "Decode estimate: 60% of 410 GB/s."
    );
    assert_eq!(draft_text(&estimate), None);
    // A count the runtime gives of one a pass, with no draft known: none.
    let mut counted = without_draft(16.14);
    counted.model.draft = None;
    counted.llm_tokens_per_pass = Some(1.0);
    assert!(!of_sample(&counted).unwrap().draft);
    // A count above one means a draft, whatever the model facts say.
    counted.llm_tokens_per_pass = Some(2.0);
    counted.llm_generation_tps = Some(32.28);
    assert!(of_sample(&counted).unwrap().draft);
    // A pass emits at least the model's own token: less is a wrong input.
    for wrong in [0.5, f64::NAN, f64::INFINITY] {
        counted.llm_tokens_per_pass = Some(wrong);
        assert_eq!(of_sample(&counted), None, "{wrong}");
    }
}

#[test]
fn unknown_inputs_leave_the_estimate_unknown() {
    let base = short();
    assert!(of_sample(&base).is_some());
    let unknown = |change: &dyn Fn(&mut Sample)| {
        let mut sample = base.clone();
        change(&mut sample);
        of_sample(&sample)
    };
    // The chip's bin, and so its peak.
    assert_eq!(unknown(&|s| s.metal.gpu_cores = None), None);
    assert_eq!(unknown(&|s| s.metal.device_name = None), None);
    // Any byte a pass reads.
    assert_eq!(unknown(&|s| s.model.bytes_per_pass = None), None);
    assert_eq!(unknown(&|s| s.model.bytes_per_pass = Some(0)), None);
    assert_eq!(unknown(&|s| s.model.kv_bytes_per_element = None), None);
    assert_eq!(unknown(&|s| s.model.fixed_state_bytes = None), None);
    // The context.
    assert_eq!(unknown(&|s| s.llm_requests[0].prompt = 0), None);
    // A rate that is not live, current or real.
    assert_eq!(unknown(&|s| s.llm_generation_tps_live = false), None);
    assert_eq!(unknown(&|s| s.llm_status = "stale".into()), None);
    assert_eq!(unknown(&|s| s.llm_source = TelemetrySource::None), None);
    assert_eq!(unknown(&|s| s.llm_generation_tps = None), None);
    assert_eq!(unknown(&|s| s.llm_generation_tps = Some(0.0)), None);
    assert_eq!(unknown(&|s| s.llm_generation_tps = Some(f64::NAN)), None);
    // A remote runtime runs on another machine's chip.
    assert_eq!(unknown(&|s| s.llm_remote = true), None);
    // Layers in CPU memory read at the CPU's pace.
    assert_eq!(unknown(&|s| s.llm_model_offloaded = Some(GIB)), None);
    assert!(unknown(&|s| s.llm_model_offloaded = Some(0)).is_some());
    // An MoE model verifying several tokens reads the experts each routes
    // to; one token a pass reads what its facts say.
    assert_eq!(unknown(&|s| s.model.experts = Some(128)), None);
    let mut moe = without_draft(16.0);
    moe.model.experts = Some(128);
    assert!(of_sample(&moe).is_some());
}

#[test]
fn only_one_request_decoding_alone_is_counted() {
    let base = short();
    let request = |id: &str, prompt: u64, prefilled: Option<u64>| RequestUsage {
        id: id.into(),
        prompt,
        output: Some(100),
        prefilled,
        ..RequestUsage::default()
    };
    // Two writing at once share a pass as the runtime decides.
    let mut two = base.clone();
    two.llm_requests.push(request("req-2", 900, None));
    assert_eq!(of_sample(&two), None);
    // A prompt read beside the answer shares the GPU.
    let mut prefill = base.clone();
    prefill
        .llm_requests
        .push(request("req-2", 9_000, Some(4_000)));
    assert_eq!(of_sample(&prefill), None);
    let mut reading = base.clone();
    reading.llm_requests[0].prefilled = Some(500);
    assert_eq!(of_sample(&reading), None);
    // Finished and queued requests read nothing.
    let mut finished = base.clone();
    let mut done = request("req-0", 50_000, None);
    done.completed = true;
    let mut queued = request("req-3", 60_000, None);
    queued.queued = true;
    finished.llm_requests.extend([done, queued]);
    assert_eq!(of_sample(&finished), of_sample(&base));
    // A runtime that lists no requests: one active, and its token counts.
    let mut unlisted = base.clone();
    unlisted.llm_requests.clear();
    unlisted.llm_prompt_tokens = Some(1_350);
    unlisted.llm_output_tokens = Some(150);
    assert_eq!(
        of_sample(&unlisted).map(|estimate| estimate.context),
        Some(1_500)
    );
    unlisted.llm_active_requests = Some(2);
    assert_eq!(of_sample(&unlisted), None);
    unlisted.llm_active_requests = Some(1);
    unlisted.llm_prompt_tokens = None;
    assert_eq!(of_sample(&unlisted), None);
    unlisted.llm_prompt_tokens = Some(0);
    unlisted.llm_output_tokens = None;
    assert_eq!(of_sample(&unlisted), None);
}

#[test]
fn the_best_is_a_high_percentile_of_usual_decode_for_similar_work() {
    let mut record = Record::default();
    let at = |speed: f64| server(speed, Some(7.8), 1_500);
    // Nineteen readings are not enough for a best.
    for _ in 0..19 {
        record.observe(&at(120.0));
    }
    let estimate = of_sample(&at(120.0)).unwrap();
    assert_eq!(record.best(&at(120.0), &estimate), None);
    assert!(!record.near_best(&at(120.0), &estimate));
    // One lucky poll does not set it; nine in ten stay under it.
    record.observe(&at(129.0));
    for _ in 0..9 {
        record.observe(&at(120.0));
    }
    let best = record.best(&at(120.0), &estimate).unwrap();
    assert!((best - estimate.share).abs() < 1e-12, "{best}");
    // Slowed samples are not what the machine can do.
    for insight in [
        CorrelationInsight {
            slow: true,
            ..CorrelationInsight::default()
        },
        CorrelationInsight {
            episode: Some(DropEpisode::default()),
            ..CorrelationInsight::default()
        },
        CorrelationInsight {
            direction: ThroughputDirection::Down,
            cause: CorrelationCause::Paging,
            ..CorrelationInsight::default()
        },
    ] {
        let mut fast = at(129.0);
        fast.correlation = insight;
        for _ in 0..30 {
            record.observe(&fast);
        }
    }
    assert_eq!(record.best(&at(120.0), &estimate), Some(best));
    // Work more than twice as deep or shallow is not similar.
    let deep = server(60.0, Some(4.4), 40_000);
    let deep_estimate = of_sample(&deep).unwrap();
    assert_eq!(record.best(&deep, &deep_estimate), None);
    for _ in 0..20 {
        record.observe(&deep);
    }
    assert!(record.best(&deep, &deep_estimate).is_some());
    assert_eq!(record.best(&at(120.0), &estimate), Some(best));
}

#[test]
fn decode_is_near_its_best_when_its_latest_middle_is_within_a_tenth() {
    let usual = server(122.8, Some(7.8), 1_500);
    let mut record = seen(&usual, 30);
    let estimate = of_sample(&usual).unwrap();
    assert!(record.near_best(&usual, &estimate));
    // One jumpy poll does not take the note down; a lasting drop does.
    let slower = server(105.0, Some(7.8), 1_500);
    record.observe(&slower);
    assert!(record.near_best(&slower, &of_sample(&slower).unwrap()));
    for _ in 0..2 {
        record.observe(&slower);
    }
    assert!(!record.near_best(&slower, &of_sample(&slower).unwrap()));
    // Another model's readings are not this one's.
    let mut other = usual.clone();
    other.llm_model = "Qwen3.8-27B-oQ6e-mtp".into();
    assert!(!record.near_best(&other, &estimate));
    assert_eq!(record.best(&other, &estimate), None);
    // A new model starts the record over.
    record.observe(&other);
    assert_eq!(record.best(&usual, &estimate), None);
}

#[test]
fn the_record_keeps_about_fifty_minutes_of_readings() {
    let mut record = seen(&short(), 3_010);
    assert_eq!(record.readings.len(), 3_000);
    record.observe(&short());
    assert_eq!(record.readings.len(), 3_000);
}

#[test]
fn the_session_summary_gives_usual_decode_by_context() {
    assert!(Record::default().summary().is_empty());
    // Readings, but no band with enough of them: no header alone.
    assert!(seen(&short(), 19).summary().is_empty());
    let mut record = seen(&short(), 25);
    for _ in 0..25 {
        record.observe(&server(51.6, Some(4.5), 80_000));
    }
    // A band with too few readings has no usual figures.
    for _ in 0..5 {
        record.observe(&server(62.1, Some(4.34), 24_000));
    }
    assert_eq!(
        record.summary(),
        [
            "Decode per verify pass by context (estimate, draft excluded):",
            "  under 4k  58% of 410 GB/s · usual 123 tok/s · 7.8 tokens per pass",
            "  64k–256k  50% of 410 GB/s · usual 52 tok/s · 4.5 tokens per pass",
        ]
    );
    // Without a draft, speed is per token and there are no passes to count.
    let record = seen(&without_draft(9.5), 20);
    assert_eq!(
        record.summary(),
        [
            "Decode by context (estimate):",
            "  under 4k  35% of 410 GB/s · usual 9.5 tok/s",
        ]
    );
}

#[test]
fn every_wording_is_one_sentence_whole_at_80_columns() {
    // The widest figures: a whole peak, a four-digit bandwidth, a slow
    // ceiling and a long band.
    let mut ultra = server(85.0, Some(9.9), 300_000);
    ultra.metal.device_name = Some("Apple M5 Ultra".into());
    ultra.metal.gpu_cores = Some(80);
    ultra.model.bytes_per_pass = Some(100_000_000_000);
    ultra.model.kv_bytes_per_element = Some(4.0);
    let at = |speed: f64| {
        let mut sample = ultra.clone();
        sample.llm_generation_tps = Some(speed);
        sample
    };
    let peak = 9.9 * 1200e9 / (100e9 + 160_038_912.0 + 300_000.0 * 131_072.0);
    let wide = at(peak * 0.999);
    let estimate = of_sample(&wide).unwrap();
    let record = seen(&wide, 20);
    let best = record.best(&wide, &estimate);
    // Diagnostics: 78 cells inside its border at 80 columns.
    let share = share_line(&estimate, best);
    assert_eq!(
        share,
        "Decode estimate: 100% of 1200 GB/s per verify pass, draft excluded; best 100%."
    );
    let draft = format!("Draft: {}.", draft_text(&estimate).unwrap());
    assert_eq!(
        draft,
        "Draft: 9.9 tokens per pass; without it, at most an estimated 8.6 tok/s."
    );
    // The session summary: 74 cells inside its popup at 80 columns.
    let summary = record.summary();
    assert_eq!(
        summary[1],
        "  256k+     100% of 1200 GB/s · usual 85 tok/s · 9.9 tokens per pass"
    );
    // `--once`: the label column and the text within 80 columns.
    let mut once = Vec::new();
    crate::report::write_static(&mut once, &wide, 1, Thresholds::default(), Platform::MacOs)
        .unwrap();
    let once = String::from_utf8(once).unwrap();
    let decode: Vec<&str> = once
        .lines()
        .filter(|line| line.starts_with("DECODE") || line.starts_with("DRAFT"))
        .collect();
    assert_eq!(decode.len(), 2, "{once}");
    // The assessment's note: 77 cells after its indent on the Overview,
    // and 72 after `Note: ` in Diagnostics, at 80 columns.
    let notes = [
        "Decode is near its estimated limit: fewer bytes/token or a better draft.",
        "Decode is near its estimated limit: fewer bytes/token or a draft model.",
    ];
    let width = |text: &str| text.chars().count();
    assert!(width(&share) <= 78 && width(&draft) <= 78);
    assert!(summary.iter().all(|line| width(line) <= 74), "{summary:?}");
    assert!(decode.iter().all(|line| width(line) <= 80), "{decode:?}");
    assert!(notes.iter().all(|note| width(note) <= 72));
    // One sentence each.
    for text in notes
        .iter()
        .copied()
        .chain([share.as_str(), draft.as_str()])
    {
        assert_eq!(text.matches(". ").count(), 0, "{text}");
        assert!(text.ends_with('.'), "{text}");
    }
}

#[test]
fn diagnostics_and_once_say_the_share_the_best_and_the_drafts_payoff() {
    let sample = short();
    let record = seen(&sample, 30);
    let lines = crate::report::diagnostic_lines(&sample, Some(&record));
    let expected = [
        "Note: Decode is near its estimated limit: fewer bytes/token or a better draft.",
        "Decode estimate: 58% of 410 GB/s per verify pass, draft excluded; best 58%.",
        "Draft: 7.8 tokens per pass; without it, at most an estimated 27 tok/s.",
    ];
    for line in expected {
        assert_eq!(
            lines.iter().filter(|shown| *shown == line).count(),
            1,
            "{line}\n{lines:#?}"
        );
    }
    // `mlxtop doctor` has watched no session: the share, no best, no note.
    let doctor = crate::report::diagnostic_lines(&sample, None);
    assert!(doctor
        .contains(&"Decode estimate: 58% of 410 GB/s per verify pass, draft excluded.".to_owned()));
    assert!(!doctor.iter().any(|line| line.starts_with("Note:")));
    // `--once` beside the usual speed, the draft's payoff under it.
    let mut once = sample.clone();
    once.correlation.usual_speed = Some((122.8, 98.2));
    once.correlation.usual_for = "1 request · 1.5k context".into();
    let mut out = Vec::new();
    crate::report::write_static(&mut out, &once, 1, Thresholds::default(), Platform::MacOs)
        .unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains(
            "USUAL SPEED   122.8 tok/s for 1 request · 1.5k context · slow below 98.2\n\
             DECODE       estimate 58% of 410 GB/s per verify pass, draft excluded\n\
             DRAFT        7.8 tokens per pass; without it, at most an estimated 27 tok/s\n"
        ),
        "{out}"
    );
    // A healthy verdict there notes nothing: `--once` has seen no best.
    assert!(!out.contains("near its estimated limit"), "{out}");
    // Without a draft: one line, per token.
    let mut out = Vec::new();
    crate::report::write_static(
        &mut out,
        &without_draft(16.14),
        1,
        Thresholds::default(),
        Platform::MacOs,
    )
    .unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("DECODE       estimate 60% of 410 GB/s\n"),
        "{out}"
    );
    assert!(!out.contains("DRAFT"), "{out}");
}

#[test]
fn a_healthy_verdict_notes_decode_near_its_best() {
    let sample = short();
    let record = seen(&sample, 30);
    let finding = crate::diagnosis::assess_session(&sample, &record);
    assert_eq!(finding.title, "Healthy · no bottleneck");
    assert_eq!(
        finding.next,
        "Decode is near its estimated limit: fewer bytes/token or a better draft."
    );
    // A note, not a check, in the verdict's own colour: nothing is wrong.
    assert!(!finding.actionable);
    assert_eq!(finding.tone, crate::domain::Tone::Green);
    // Without a draft, a draft model is the other way to go faster.
    let plain = without_draft(16.14);
    assert_eq!(
        crate::diagnosis::assess_session(&plain, &seen(&plain, 30)).next,
        "Decode is near its estimated limit: fewer bytes/token or a draft model."
    );
    // No note before a best, nor far below it.
    assert_eq!(
        crate::diagnosis::assess_session(&sample, &seen(&sample, 19)).next,
        ""
    );
    let mut record = seen(&sample, 30);
    let slower = server(100.0, Some(7.8), 1_500);
    for _ in 0..5 {
        record.observe(&slower);
    }
    assert_eq!(crate::diagnosis::assess_session(&slower, &record).next, "");
    // Nor while generation is slowed, slow, not yet judged, or the verdict
    // is anything but healthy.
    let mut slowed = sample.clone();
    slowed.correlation.direction = ThroughputDirection::Down;
    slowed.correlation.cause = CorrelationCause::None;
    let finding = crate::diagnosis::assess_session(&slowed, &record);
    assert_ne!(finding.title, "Healthy · no bottleneck");
    assert!(!finding.next.contains("estimated limit"));
    let mut slow = sample.clone();
    slow.correlation.slow = true;
    assert_eq!(crate::diagnosis::assess_session(&slow, &record).next, "");
    let mut learning = sample.clone();
    learning.correlation.direction = ThroughputDirection::Unknown;
    assert_eq!(
        crate::diagnosis::assess_session(&learning, &record).next,
        "Waiting for a live throughput baseline."
    );
    let mut pressed = sample.clone();
    pressed.pressure = "YELLOW".into();
    pressed.rate_ready = true;
    pressed.vm_available = true;
    assert_eq!(crate::diagnosis::assess_session(&pressed, &record).next, "");
}

#[test]
fn the_session_summary_puts_decode_after_the_speed_lost() {
    let sample = short();
    let mut session = crate::session::Session::default();
    for _ in 0..20 {
        session.observe(&sample);
    }
    let lines = crate::session::lines(
        &session,
        &crate::request_history::History::default(),
        &sample,
        None,
    );
    let lost = lines
        .iter()
        .position(|line| line.starts_with("Speed lost"))
        .unwrap();
    assert_eq!(
        lines[lost + 1],
        "Decode per verify pass by context (estimate, draft excluded):"
    );
    assert!(lines[lost + 2].starts_with("  under 4k  58% of 410 GB/s"));
}

#[test]
fn percentiles_fall_between_the_nearest_readings() {
    assert_eq!(percentile(Vec::new(), 0.5), None);
    assert_eq!(percentile(vec![3.0], 0.9), Some(3.0));
    assert_eq!(percentile(vec![4.0, 1.0, 3.0, 2.0], 0.5), Some(2.5));
    let tens: Vec<f64> = (1..=11).map(f64::from).collect();
    assert_eq!(percentile(tens.clone(), 0.9), Some(10.0));
    assert_eq!(percentile(tens, 2.0), Some(11.0));
}
