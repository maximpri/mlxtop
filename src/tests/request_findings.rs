// SPDX-License-Identifier: MIT
use super::*;
use crate::domain::RequestTiming;
use std::time::{Duration, SystemTime};

const MODEL: &str = "Qwen3.8-27B-oQ4e-mtp";

fn at(second: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + second)
}

/// What a listed request is doing.
#[derive(Clone, Copy)]
enum Phase {
    Queued,
    Reading,
    Writing(f64),
    /// Reported after it ended (a usage recorder's record).
    Reported,
}

fn request(id: &str, prompt: u64, cached: Option<u64>, phase: Phase, second: u64) -> RequestUsage {
    RequestUsage {
        provider: "oMLX".into(),
        model: MODEL.into(),
        id: id.into(),
        prompt,
        cached,
        output: matches!(phase, Phase::Writing(_) | Phase::Reported).then_some(100),
        output_tps: match phase {
            Phase::Writing(tps) => Some(tps),
            _ => None,
        },
        completed: matches!(phase, Phase::Reported),
        queued: matches!(phase, Phase::Queued),
        prefilled: matches!(phase, Phase::Reading).then_some(prompt / 2),
        idle_for: None,
        ttft_ms: None,
        observed_at: Some(at(second)),
    }
}

fn sample(second: u64, requests: Vec<RequestUsage>) -> Sample {
    Sample {
        llm_provider: "oMLX".into(),
        llm_model: MODEL.into(),
        llm_observed_at: Some(at(second)),
        updated: format!("12:{:02}:{:02}", second / 60 % 60, second % 60),
        llm_requests: requests,
        ..Sample::default()
    }
}

/// The same sample from another runtime.
fn from(provider: &str, mut sample: Sample) -> Sample {
    sample.llm_provider = provider.into();
    for request in &mut sample.llm_requests {
        request.provider = provider.into();
    }
    for timing in &mut sample.llm_request_timings {
        timing.provider = provider.into();
    }
    sample
}

/// Samples through the findings as the collector passes them: the request
/// history first, then the findings, keeping every row they give.
#[derive(Default)]
struct Run {
    findings: Findings,
    history: History,
    previous: Option<Sample>,
    rows: Vec<(&'static str, String)>,
}

impl Run {
    fn step(&mut self, sample: Sample) {
        self.history.observe(&sample.llm_requests);
        let rows = self
            .findings
            .observe(self.previous.as_ref(), &sample, &self.history);
        self.rows.extend(rows);
        self.previous = Some(sample);
    }

    fn evidence(&self) -> Evidence {
        self.findings.evidence()
    }
}

/// A request of `prompt` tokens, `cached` of them from the cache, as
/// llama.cpp lists it: whole counts while it writes. One ends before the
/// next begins; a prompt read at 250 tok/s is measured on the way.
fn two_in_a_row(first: (u64, u64), second: (u64, u64)) -> Run {
    let mut run = Run::default();
    let writing = |id, (prompt, cached): (u64, u64), at| {
        from(
            "llama.cpp",
            sample(
                at,
                vec![request(id, prompt, Some(cached), Phase::Writing(50.0), at)],
            ),
        )
    };
    for at in 0..3 {
        run.step(writing("a", first, at));
    }
    for at in 3..5 {
        let mut reading = from(
            "llama.cpp",
            sample(
                at,
                vec![request("b", second.0, Some(second.1), Phase::Reading, at)],
            ),
        );
        reading.llm_prefill_tps = Some(250.0);
        reading.llm_prefill_tps_live = true;
        run.step(reading);
    }
    for at in 5..7 {
        run.step(writing("b", second, at));
    }
    // It ended; its count is final three samples later.
    for at in 7..11 {
        run.step(from("llama.cpp", sample(at, Vec::new())));
    }
    run
}

#[test]
fn a_long_prompt_read_in_full_right_after_a_cached_one_is_told_with_its_cost() {
    let run = two_in_a_row((30_000, 29_000), (31_000, 0));
    assert_eq!(
        run.rows,
        [(
            "CACHE MISS",
            "b · 31.0k prompt read in full right after one of 30.0k was 96% cached · ≈2m 4s of prompt reading"
                .to_owned()
        )]
    );
    let evidence = run.evidence();
    assert_eq!(evidence.full_reads, 1);
    assert_eq!(evidence.full_read_seconds, 124.0);
    assert_eq!(
        evidence.last_full_read,
        Some(FullRead {
            at: "12:00:10".into(),
            prompt: 31_000,
            previous: 30_000,
            previous_percent: 96,
        })
    );
    assert_eq!(run.findings.lost()[0], ("cache misses", 124.0));
}

#[test]
fn a_full_read_is_a_long_prompt_after_a_cached_one_of_similar_length() {
    for (case, first, second, told) in [
        // At the edges of each threshold.
        ("4,096 computed", (4_000, 4_000), (4_096, 0), true),
        ("under 10% cached", (30_000, 30_000), (31_000, 3_000), true),
        ("80% cached before", (30_000, 24_000), (31_000, 0), true),
        ("half as long", (60_000, 60_000), (30_000, 0), true),
        ("twice as long", (15_000, 15_000), (30_000, 0), true),
        // Past them.
        ("fewer computed", (4_000, 4_000), (4_095, 0), false),
        ("10% cached", (30_000, 30_000), (31_000, 3_100), false),
        ("less cached before", (30_000, 23_999), (31_000, 0), false),
        // An agent that compacted its history, or a new task: another
        // context, which no cache could have served.
        ("much shorter", (86_000, 86_000), (30_000, 0), false),
        ("much longer", (2_000, 2_000), (31_000, 0), false),
    ] {
        let run = two_in_a_row(first, second);
        assert_eq!(run.rows.len(), usize::from(told), "{case}: {:?}", run.rows);
    }
}

#[test]
fn omlx_counts_a_requests_cached_tokens_when_it_ends() {
    // oMLX lists no cached count beside a request: it adds each request's
    // prompt and cached tokens to its totals as the request ends.
    let mut run = Run::default();
    let totals = |mut sample: Sample, prompt: u64, cached: u64| {
        sample.llm_total_prompt_tokens = Some(prompt);
        sample.llm_total_cached_tokens = Some(cached);
        sample.llm_prefill_tps = Some(310.0);
        sample
    };
    for at in 0..3 {
        run.step(totals(
            sample(
                at,
                vec![request("a", 30_000, None, Phase::Writing(50.0), at)],
            ),
            1_000,
            900,
        ));
    }
    run.step(totals(sample(3, Vec::new()), 31_000, 29_900));
    for at in 4..6 {
        run.step(totals(
            sample(
                at,
                vec![request("b", 31_000, None, Phase::Writing(50.0), at)],
            ),
            31_000,
            29_900,
        ));
    }
    // Told as soon as the totals say it, with oMLX's own prefill figure.
    run.step(totals(sample(6, Vec::new()), 62_000, 29_900));
    assert_eq!(
        run.rows,
        [(
            "CACHE MISS",
            "b · 31.0k prompt read in full right after one of 30.0k was 96% cached · ≈1m 40s of prompt reading"
                .to_owned()
        )]
    );
}

#[test]
fn a_rise_in_omlx_totals_that_matches_no_one_request_says_nothing() {
    let mut run = Run::default();
    let totals = |mut sample: Sample, prompt: u64, cached: u64| {
        sample.llm_total_prompt_tokens = Some(prompt);
        sample.llm_total_cached_tokens = Some(cached);
        sample
    };
    let both = |at| {
        vec![
            request("a", 30_000, None, Phase::Writing(50.0), at),
            request("b", 31_000, None, Phase::Writing(50.0), at),
        ]
    };
    run.step(totals(sample(0, both(0)), 0, 0));
    run.step(totals(sample(1, both(1)), 0, 0));
    // Both end in one poll: their counts cannot be told apart.
    run.step(totals(sample(2, Vec::new()), 61_000, 30_000));
    // Nor is a total that went back, or another runtime's.
    run.step(totals(sample(3, Vec::new()), 0, 0));
    run.step(from("Splash", totals(sample(4, Vec::new()), 31_000, 0)));
    for at in 5..9 {
        run.step(sample(at, Vec::new()));
    }
    assert!(run.rows.is_empty(), "{:?}", run.rows);
    assert_eq!(run.evidence(), Evidence::default());
}

#[test]
fn a_full_read_is_news_only_for_a_request_of_this_session() {
    // A usage recorder's file holds requests that ended before mlxtop
    // started: history, though the request before a new one may be one.
    let mut run = Run::default();
    let reported =
        |id, prompt, cached, second| request(id, prompt, Some(cached), Phase::Reported, second);
    let backlog = vec![
        reported("old-cached", 30_000, 29_000, 1),
        reported("old-read", 31_000, 0, 2),
        reported("cached", 31_500, 31_000, 3),
    ];
    run.step(sample(100, backlog.clone()));
    run.step(sample(101, backlog.clone()));
    assert!(run.rows.is_empty(), "{:?}", run.rows);
    let mut now = backlog;
    now.push(reported("new", 32_000, 0, 102));
    run.step(sample(102, now));
    assert_eq!(run.rows.len(), 1, "{:?}", run.rows);
    // Unknown prefill speed: the row gives no time, and costs none.
    assert_eq!(
        run.rows[0].1,
        "new · 32.0k prompt read in full right after one of 31.5k was 98% cached"
    );
    assert_eq!(run.findings.lost()[0], ("cache misses", 0.0));
    // Nothing before it with a known count: nothing to judge it against.
    let mut alone = Run::default();
    alone.step(sample(0, Vec::new()));
    alone.step(sample(1, vec![reported("first", 32_000, 0, 1)]));
    assert!(alone.rows.is_empty(), "{:?}", alone.rows);
}

#[test]
fn the_first_token_waits_for_a_model_load_the_runtime_reports() {
    let mut run = Run::default();
    let timing = |id: &str, load_ms| RequestTiming {
        provider: "oMLX".into(),
        model: MODEL.into(),
        id: id.into(),
        prefill_tps: None,
        load_ms: Some(load_ms),
    };
    let reported = |id, second| request(id, 900, Some(0), Phase::Reported, second);
    let mut first = sample(10, vec![reported("before", 5)]);
    first.llm_request_timings = vec![timing("before", 30_000)];
    run.step(first);
    for at in 11..14 {
        let mut next = sample(
            at,
            vec![
                reported("before", 5),
                reported("cold", 11),
                reported("warm", 12),
            ],
        );
        next.llm_request_timings = vec![
            timing("before", 30_000),
            timing("cold", 6_500),
            // Ollama reports a few milliseconds for a model it holds.
            timing("warm", 999),
        ];
        run.step(next);
    }
    assert_eq!(
        run.rows,
        [(
            "MODEL LOAD",
            "cold · first token waited 6.5s for the model to load".to_owned()
        )]
    );
    let evidence = run.evidence();
    assert_eq!(
        (evidence.loads, evidence.load_seconds),
        (1, 6.5),
        "{evidence:?}"
    );
    assert_eq!(evidence.longest_load, Some(("12:00:11".into(), 6.5)));
    assert_eq!(run.findings.lost()[1], ("model loads", 6.5));
}

#[test]
fn requests_recomputed_for_a_full_kv_cache_are_told_once_a_story() {
    let mut run = Run::default();
    let count = |second, provider: &str, total| {
        let mut sample = from(provider, sample(second, Vec::new()));
        sample.llm_preemptions_total = Some(total);
        sample
    };
    run.step(count(0, "vLLM", 5));
    run.step(count(1, "vLLM", 8));
    // A rise within a minute is the same story.
    run.step(count(2, "vLLM", 9));
    run.step(count(70, "vLLM", 10));
    // A counter that went back is a restarted runtime, not news; another
    // runtime's count is its own.
    run.step(count(71, "vLLM", 3));
    run.step(count(72, "SGLang", 50));
    run.step(count(73, "SGLang", 50));
    assert_eq!(
        run.rows,
        [
            ("KV CACHE FULL", "3 requests were recomputed".to_owned()),
            ("KV CACHE FULL", "1 request was recomputed".to_owned()),
        ]
    );
    let evidence = run.evidence();
    assert_eq!(evidence.recomputed, 5);
    assert_eq!(evidence.last_recomputed.as_deref(), Some("12:01:10"));
}

/// A session of requests that start alone or while another writes:
/// twelve alone (two seconds reading, four writing at 120 tok/s), then
/// `pairs` pairs. In each, `a` starts alone and writes at 120 tok/s; `b`
/// arrives while it writes and reads its prompt for `wait` seconds; both
/// write at `shared` tok/s for `together` seconds; then `b` alone at 120.
fn overlapping(pairs: u64, shared: f64, together: u64, wait: u64, limit: Option<u64>) -> Run {
    let mut run = Run::default();
    let mut second = 0;
    let mut step = |run: &mut Run, mut requests: Vec<RequestUsage>| {
        for request in &mut requests {
            request.observed_at = Some(at(second));
        }
        let mut next = sample(second, requests);
        next.llm_max_concurrent = limit;
        run.step(next);
        second += 1;
    };
    for n in 0..12 {
        let id = format!("alone-{n}");
        for _ in 0..2 {
            step(&mut run, vec![request(&id, 900, None, Phase::Reading, 0)]);
        }
        for _ in 0..4 {
            step(
                &mut run,
                vec![request(&id, 900, None, Phase::Writing(120.0), 0)],
            );
        }
    }
    step(&mut run, Vec::new());
    for n in 0..pairs {
        let (a, b) = (format!("a-{n}"), format!("b-{n}"));
        let writing = |id: &str, tps| request(id, 900, None, Phase::Writing(tps), 0);
        for _ in 0..2 {
            step(&mut run, vec![request(&a, 900, None, Phase::Reading, 0)]);
        }
        for _ in 0..3 {
            step(&mut run, vec![writing(&a, 120.0)]);
        }
        for _ in 0..wait {
            step(
                &mut run,
                vec![
                    writing(&a, 120.0),
                    request(&b, 900, None, Phase::Reading, 0),
                ],
            );
        }
        for _ in 0..together {
            step(&mut run, vec![writing(&a, shared), writing(&b, shared)]);
        }
        for _ in 0..3 {
            step(&mut run, vec![writing(&b, 120.0)]);
        }
        step(&mut run, Vec::new());
    }
    run
}

#[test]
fn two_requests_at_once_that_gain_nothing_and_delay_first_tokens_are_told_once() {
    let run = overlapping(10, 58.0, 20, 6, Some(1));
    let told: Vec<_> = run
        .rows
        .iter()
        .filter(|(state, _)| *state == "OVERLAP")
        .collect();
    assert_eq!(
        told,
        [&(
            "OVERLAP",
            "two requests at once gained no throughput (0.97× one at a time) and made first tokens wait 3.0× as long · oMLX is set to run 1 at a time"
                .to_owned()
        )]
    );
    assert_eq!(run.rows.len(), 1, "{:?}", run.rows);
    let overlap = run.evidence().overlap.unwrap();
    assert!((overlap.payoff - 58.0 / 60.0).abs() < 1e-9, "{overlap:?}");
    assert_eq!(
        (
            overlap.seconds,
            overlap.pairs,
            overlap.beside,
            overlap.alone
        ),
        (200.0, 10, 6.0, 2.0)
    );
    assert_eq!((overlap.beside_requests, overlap.alone_requests), (10, 22));
    // Each started beside another waited 4 s beyond the usual 2.
    assert_eq!(overlap.extra_wait, 40.0);
    assert_eq!(run.findings.lost()[2], ("overlap", 40.0));
    assert_eq!(
        run.evidence().limit,
        Some(Limit {
            provider: "oMLX".into(),
            setting: "max_concurrent_requests",
            configured: 1,
            observed: 2,
        })
    );
}

#[test]
fn overlap_is_not_told_when_it_pays_or_costs_no_first_tokens_or_is_barely_seen() {
    // Two at once at 100 tok/s each: 1.67× one at a time, worth it.
    let pays = overlapping(10, 100.0, 20, 6, None);
    assert!(pays.rows.is_empty(), "{:?}", pays.rows);
    let overlap = pays.evidence().overlap.unwrap();
    assert!((overlap.payoff - 200.0 / 120.0).abs() < 1e-9);
    assert!(!overlap.buys_nothing());
    assert_eq!(pays.findings.lost()[2], ("overlap", 0.0));
    assert_eq!(pays.evidence().limit, None);
    // First tokens wait as long beside another as alone.
    let prompt = overlapping(10, 58.0, 20, 2, None);
    assert!(prompt.rows.is_empty(), "{:?}", prompt.rows);
    // Too little evidence: nine requests beside another, or under a minute
    // of two writing at once, or too few pairs.
    for (pairs, together) in [(9, 20), (10, 5)] {
        let thin = overlapping(pairs, 58.0, together, 6, None);
        assert!(thin.rows.is_empty(), "{:?}", thin.rows);
        assert_eq!(thin.evidence().overlap, None);
    }
}

#[test]
fn a_wait_counts_only_for_a_request_watched_before_it_wrote() {
    let mut run = Run::default();
    // First seen writing: when its prompt was read is unknown.
    run.step(sample(
        0,
        vec![request("seen-late", 900, None, Phase::Writing(120.0), 0)],
    ));
    run.step(sample(
        1,
        vec![request("seen-late", 900, None, Phase::Writing(120.0), 1)],
    ));
    // Queued with nothing running: it started alone, and waited 2 s.
    run.step(sample(
        2,
        vec![request("queued", 900, None, Phase::Queued, 2)],
    ));
    run.step(sample(
        3,
        vec![request("queued", 900, None, Phase::Reading, 3)],
    ));
    run.step(sample(
        4,
        vec![request("queued", 900, None, Phase::Writing(120.0), 4)],
    ));
    // Two that start reading their prompts together: neither alone nor
    // beside another writing.
    run.step(sample(
        5,
        vec![
            request("one", 900, None, Phase::Reading, 5),
            request("two", 900, None, Phase::Reading, 5),
        ],
    ));
    // A runtime that reports its first token: that wait, whatever was
    // watched. It started while `one` wrote.
    let mut reported = request("reported", 900, None, Phase::Writing(120.0), 6);
    reported.ttft_ms = Some(1_500);
    run.step(sample(
        6,
        vec![
            request("one", 900, None, Phase::Writing(120.0), 6),
            reported,
        ],
    ));
    let waits = |start: Start| -> Vec<Option<f64>> {
        run.findings
            .requests
            .iter()
            .filter(|seen| seen.start == start)
            .map(|seen| seen.first_token)
            .collect()
    };
    assert_eq!(waits(Start::Alone), [None, Some(2.0)]);
    assert_eq!(waits(Start::Beside), [Some(1.5)]);
    assert_eq!(waits(Start::Other), [Some(1.0), None]);
    assert_eq!(
        run.findings.alone.iter().copied().collect::<Vec<_>>(),
        [2.0]
    );
    assert_eq!(
        run.findings.beside.iter().copied().collect::<Vec<_>>(),
        [1.5]
    );
}

#[test]
fn a_runtime_set_to_run_fewer_requests_than_ran_is_noted() {
    let mut run = Run::default();
    let running = |second, provider: &str, n: u64, slots: Option<u64>| {
        let requests = (0..n)
            .map(|i| request(&format!("r{i}"), 900, None, Phase::Writing(50.0), second))
            .collect();
        let mut sample = from(provider, sample(second, requests));
        sample.llama.slots = slots;
        sample
    };
    run.step(running(0, "llama.cpp", 2, Some(1)));
    assert_eq!(
        run.evidence().limit,
        Some(Limit {
            provider: "llama.cpp".into(),
            setting: "--parallel",
            configured: 1,
            observed: 2,
        })
    );
    // Within its slots, nothing to note; another runtime starts the count again.
    run.step(running(1, "llama.cpp", 1, Some(4)));
    assert_eq!(run.evidence().limit, None);
    run.step(running(2, "LM Studio", 1, None));
    run.step(running(3, "oMLX", 1, None));
    assert_eq!(run.evidence().limit, None);
    // A runtime that lists no requests counts its active ones.
    let mut counted = from("oMLX", sample(4, Vec::new()));
    counted.llm_active_requests = Some(3);
    counted.llm_max_concurrent = Some(2);
    run.step(counted);
    assert_eq!(run.evidence().limit.map(|limit| limit.observed), Some(3));
}

#[test]
fn diagnostics_give_the_evidence_and_nothing_without_it() {
    assert!(lines(&Evidence::default()).is_empty());
    let evidence = Evidence {
        full_reads: 2,
        full_read_seconds: 192.0,
        last_full_read: Some(FullRead {
            at: "14:02:11".into(),
            prompt: 45_000,
            previous: 86_300,
            previous_percent: 100,
        }),
        loads: 2,
        load_seconds: 12.4,
        longest_load: Some(("14:05:00".into(), 8.2)),
        overlap: Some(Overlap {
            payoff: 0.94,
            seconds: 272.0,
            pairs: 5,
            beside: 8.9,
            alone: 3.2,
            beside_requests: 47,
            alone_requests: 230,
            extra_wait: 287.0,
        }),
        limit: Some(Limit {
            provider: "oMLX".into(),
            setting: "max_concurrent_requests",
            configured: 1,
            observed: 2,
        }),
        recomputed: 3,
        last_recomputed: Some("14:25:00".into()),
    };
    assert_eq!(
        lines(&evidence),
        [
            "Cache misses: 2 long prompts read in full right after cached ones, ≈3m 12s of prompt reading; the last at 14:02:11, 45.0k after one of 86.3k was 100% cached",
            "Model loads: first tokens waited 12s for 2 loads; the longest 8.2s at 14:05:00",
            "Two requests at once: 0.94× the work of one at a time over 4m 32s (5 pairs); first token median 8.9s when started beside another writing, 3.2s alone (47 and 230 requests)",
            "oMLX is set to run 1 request at a time (max_concurrent_requests), yet 2 ran at once",
            "KV cache full: 3 requests recomputed this session, the last at 14:25:00",
        ]
    );
    // One of each, with no reading time known.
    let one = Evidence {
        full_reads: 1,
        full_read_seconds: 0.0,
        loads: 1,
        load_seconds: 1.0,
        longest_load: Some(("09:00:00".into(), 1.0)),
        overlap: None,
        limit: Some(Limit {
            provider: "llama.cpp".into(),
            setting: "--parallel",
            configured: 2,
            observed: 3,
        }),
        recomputed: 1,
        last_recomputed: None,
        ..evidence
    };
    assert_eq!(
        lines(&one),
        [
            "Cache misses: 1 long prompt read in full right after cached ones; the last at 14:02:11, 45.0k after one of 86.3k was 100% cached",
            "Model loads: first tokens waited 1.0s for 1 load; the longest 1.0s at 09:00:00",
            "llama.cpp is set to run 2 requests at a time (--parallel), yet 3 ran at once",
            "KV cache full: 1 request recomputed this session",
        ]
    );
}

#[test]
fn the_summary_gives_what_first_tokens_lost_by_cause() {
    assert!(summary(&Findings::default()).is_empty());
    let run = overlapping(10, 58.0, 20, 6, None);
    assert_eq!(
        summary(&run.findings),
        [
            "Time lost before first tokens: 40s",
            "  overlap                    40s",
        ]
    );
    let read = two_in_a_row((30_000, 29_000), (31_000, 0));
    let mut findings = read.findings;
    findings.evidence.load_seconds = 0.4;
    assert_eq!(
        summary(&findings),
        [
            "Time lost before first tokens: 2m 4s",
            "  cache misses               2m 4s",
        ]
    );
}

#[test]
fn seconds_read_to_a_tenth_under_ten() {
    assert_eq!(seconds_label(6.53), "6.5s");
    assert_eq!(seconds_label(9.94), "9.9s");
    assert_eq!(seconds_label(10.0), "10s");
    assert_eq!(seconds_label(176.0), "2m 56s");
}

#[test]
fn a_session_begins_by_this_machines_clock_not_the_newest_record() {
    // A usage file's newest record sets the runtime's observation time;
    // the session began when mlxtop sampled, and the records are older.
    let mut run = Run::default();
    let reported =
        |id, prompt, cached, second| request(id, prompt, Some(cached), Phase::Reported, second);
    let backlog = vec![
        reported("cached", 30_000, 29_000, 1),
        reported("read", 31_000, 0, 2),
    ];
    for second in 100..105 {
        let mut polled = sample(second, backlog.clone());
        polled.llm_observed_at = Some(at(2));
        polled.ts = Some(
            at(second)
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        );
        run.step(polled);
    }
    assert!(run.rows.is_empty(), "{:?}", run.rows);
}

#[test]
fn a_prefill_speed_a_runtime_reports_prices_a_full_read() {
    // A usage recorder gives each finished request's prompt speed, and no
    // live prefill is seen: the reported speeds are this machine's.
    let mut run = Run::default();
    let reported =
        |id, prompt, cached, second| request(id, prompt, Some(cached), Phase::Reported, second);
    let timing = |id: &str, speed| RequestTiming {
        provider: "oMLX".into(),
        model: MODEL.into(),
        id: id.into(),
        prefill_tps: Some(speed),
        load_ms: None,
    };
    run.step(sample(0, Vec::new()));
    let mut first = sample(1, vec![reported("cached", 30_000, 29_000, 1)]);
    first.llm_request_timings = vec![timing("cached", 400.0)];
    run.step(first);
    let mut second = sample(
        2,
        vec![
            reported("cached", 30_000, 29_000, 1),
            reported("read", 32_000, 0, 2),
        ],
    );
    // A timing for a request the runtime no longer lists says nothing.
    second.llm_request_timings = vec![
        timing("cached", 400.0),
        timing("read", 400.0),
        RequestTiming {
            load_ms: Some(9_000),
            ..timing("gone", 400.0)
        },
    ];
    run.step(second);
    assert_eq!(
        run.rows,
        [(
            "CACHE MISS",
            "read · 32.0k prompt read in full right after one of 30.0k was 96% cached · ≈1m 20s of prompt reading"
                .to_owned()
        )]
    );
    assert_eq!(run.findings.prefill_speeds.len(), 2);
}

#[test]
fn two_writers_are_one_pair_in_either_order_and_need_both_speeds() {
    let mut run = Run::default();
    let writing = |id, tps| request(id, 900, None, Phase::Writing(tps), 0);
    let mut step = |second: u64, requests: Vec<RequestUsage>| {
        let mut polled = sample(second, requests);
        for request in &mut polled.llm_requests {
            request.observed_at = Some(at(second));
        }
        run.step(polled);
    };
    step(0, vec![writing("b", 60.0), writing("a", 50.0)]);
    step(1, vec![writing("b", 60.0), writing("a", 50.0)]);
    step(2, vec![writing("a", 50.0), writing("b", 60.0)]);
    // A writer whose own speed is not known this poll adds nothing.
    let mut unknown = writing("b", 0.0);
    unknown.output_tps = None;
    step(3, vec![writing("a", 50.0), unknown]);
    let pairs = &run.findings.pairs;
    assert_eq!(pairs.len(), 1);
    let pair = &pairs[0];
    assert_eq!((pair.a.2.as_str(), pair.b.2.as_str()), ("a", "b"));
    assert_eq!(
        (pair.tokens_a, pair.tokens_b, pair.seconds),
        (100.0, 120.0, 2.0)
    );
}

#[test]
fn what_is_kept_is_bounded() {
    let mut run = Run::default();
    for second in 0..=SPEEDS_KEPT as u64 {
        let mut reading = sample(second, Vec::new());
        reading.llm_prefill_tps = Some(100.0 + second as f64);
        reading.llm_prefill_tps_live = true;
        run.step(reading);
    }
    assert_eq!(run.findings.prefill_speeds.len(), SPEEDS_KEPT);
    assert_eq!(run.findings.prefill_speeds.front(), Some(&101.0));
}
