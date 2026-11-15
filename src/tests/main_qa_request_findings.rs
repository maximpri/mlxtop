// SPDX-License-Identifier: MIT
//! The QA judge on what requests lose before their first tokens: Journal
//! rows for a prompt read in full after a cached one, a model load, a full
//! KV cache and two requests at once that bought nothing; Diagnostics'
//! evidence; the session summary's time lost. Each at every size, with the
//! widest figures and with nothing to show. None of it may colour.
use super::qa::{judge, Case, SIZES};
use super::*;
use crate::domain::RequestTiming;
use crate::request_findings::{Evidence, Findings, FullRead, Limit, Overlap};
use crate::request_history::History;

const MODEL: &str = "Qwen3.8-27B-oQ4e-mtp";

/// The four rows a session with every finding writes.
const ROWS: [(&str, &str); 4] = [
    (
        "OVERLAP",
        "two requests at once gained no throughput (0.97× one at a time) and made first tokens wait 3.0× as long · oMLX is set to run 1 at a time",
    ),
    (
        "CACHE MISS",
        "b · 45.0k prompt read in full right after one of 44.0k was 98% cached · ≈2m 56s of prompt reading",
    ),
    (
        "MODEL LOAD",
        "cold · first token waited 6.5s for the model to load",
    ),
    ("KV CACHE FULL", "3 requests were recomputed"),
];

fn at(second: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + second)
}

/// A request oMLX lists: reading its prompt, or writing at `tps`.
fn listed(id: &str, prompt: u64, tps: Option<f64>, second: u64) -> RequestUsage {
    RequestUsage {
        provider: "oMLX".into(),
        model: MODEL.into(),
        id: id.into(),
        prompt,
        output: tps.map(|_| 400),
        output_tps: tps,
        prefilled: tps.is_none().then_some(prompt / 2),
        observed_at: Some(at(second)),
        ..RequestUsage::default()
    }
}

/// A session through the collector as it concludes each sample it reads:
/// two requests at once ten times over, each pair at 58 tok/s together
/// against 120 alone, the second of each waiting 6 s for its first token
/// against 2 s alone; a 45k-token prompt read in full after a 44k one 98%
/// cached; a model load; three requests recomputed. oMLX is set to run one
/// request at a time.
fn session() -> Collector {
    let mut collector = Collector::with_host(
        600,
        offline_config(),
        Box::new(FakeHost::default()),
        Platform::MacOs,
        None,
    );
    let mut second = 0;
    let mut totals = (0, 0);
    let mut poll =
        |collector: &mut Collector, requests: Vec<RequestUsage>, change: &dyn Fn(&mut Sample)| {
            let mut sample = Sample {
                llm_provider: "oMLX".into(),
                llm_model: MODEL.into(),
                llm_source: TelemetrySource::Live,
                llm_status: "generating".into(),
                llm_observed_at: Some(at(second)),
                updated: format!("14:{:02}:{:02}", second / 60 % 60, second % 60),
                llm_requests: requests
                    .into_iter()
                    .map(|mut request| {
                        request.observed_at = Some(at(second));
                        request
                    })
                    .collect(),
                llm_max_concurrent: Some(1),
                llm_preemptions_total: Some(0),
                ..Sample::default()
            };
            change(&mut sample);
            collector.conclude(sample, Instant::now());
            second += 1;
        };
    let same = |_: &mut Sample| {};
    for n in 0..12 {
        let id = format!("alone-{n}");
        for _ in 0..2 {
            poll(&mut collector, vec![listed(&id, 900, None, 0)], &same);
        }
        for _ in 0..4 {
            poll(
                &mut collector,
                vec![listed(&id, 900, Some(120.0), 0)],
                &same,
            );
        }
    }
    for n in 0..10 {
        let (a, b) = (format!("a-{n}"), format!("b-{n}"));
        for _ in 0..2 {
            poll(&mut collector, vec![listed(&a, 900, None, 0)], &same);
        }
        for _ in 0..3 {
            poll(&mut collector, vec![listed(&a, 900, Some(120.0), 0)], &same);
        }
        for _ in 0..6 {
            poll(
                &mut collector,
                vec![listed(&a, 900, Some(120.0), 0), listed(&b, 900, None, 0)],
                &same,
            );
        }
        for _ in 0..20 {
            poll(
                &mut collector,
                vec![
                    listed(&a, 900, Some(58.0), 0),
                    listed(&b, 900, Some(58.0), 0),
                ],
                &same,
            );
        }
        for _ in 0..3 {
            poll(&mut collector, vec![listed(&b, 900, Some(120.0), 0)], &same);
        }
    }
    // oMLX adds each request's prompt and cached tokens to its totals as
    // it ends; prompts are read at 255 tok/s.
    let mut counted = |collector: &mut Collector, requests, prompt: u64, cached: u64| {
        totals = (totals.0 + prompt, totals.1 + cached);
        let (sent, served) = totals;
        poll(collector, requests, &move |sample: &mut Sample| {
            sample.llm_total_prompt_tokens = Some(sent);
            sample.llm_total_cached_tokens = Some(served);
            sample.llm_prefill_tps = Some(255.0);
            sample.llm_prefill_tps_live = true;
        });
    };
    counted(
        &mut collector,
        vec![listed("a", 44_000, Some(60.0), 0)],
        0,
        0,
    );
    counted(
        &mut collector,
        vec![listed("a", 44_000, Some(60.0), 0)],
        0,
        0,
    );
    counted(&mut collector, Vec::new(), 44_000, 43_200);
    counted(&mut collector, vec![listed("b", 45_000, None, 0)], 0, 0);
    counted(
        &mut collector,
        vec![listed("b", 45_000, Some(60.0), 0)],
        0,
        0,
    );
    counted(&mut collector, Vec::new(), 45_000, 0);
    // A usage recorder's record of a request that waited for its model.
    let cold = RequestUsage {
        completed: true,
        ..listed("cold", 1_200, Some(40.0), 0)
    };
    for _ in 0..2 {
        poll(
            &mut collector,
            vec![cold.clone()],
            &|sample: &mut Sample| {
                sample.llm_request_timings = vec![RequestTiming {
                    provider: "oMLX".into(),
                    model: MODEL.into(),
                    id: "cold".into(),
                    prefill_tps: None,
                    load_ms: Some(6_500),
                }];
            },
        );
    }
    poll(&mut collector, Vec::new(), &|sample: &mut Sample| {
        sample.llm_preemptions_total = Some(3);
    });
    collector
}

/// The dashboard fixture with the session's findings: its rows (with the
/// requests' own rows around them), what Diagnostics shows and what the
/// summary adds.
fn with_findings(view: char) -> App {
    let collector = session();
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    app.collector.signals = collector
        .signals
        .iter()
        .filter(|row| {
            ROWS.iter().any(|(state, _)| row.state == *state)
                || row.summary.starts_with("b ")
                || row.summary.starts_with("cold ")
        })
        .cloned()
        .collect();
    app.collector.session.request_findings = collector.session.request_findings.clone();
    app.collector.current.request_findings = collector.current.request_findings.clone();
    open(app, view)
}

fn open(mut app: App, view: char) -> App {
    match view {
        'd' => app.diagnostics_open = true,
        'u' => app.summary_open = true,
        'j' => app.tab = 2,
        _ => {}
    }
    app
}

/// The widest figures: a 1.2M-token prompt read in full at 100 tok/s, an
/// hour's load, a thousand requests recomputed, hours of two at once.
fn widest(view: char) -> App {
    let mut findings = Findings::default();
    let mut history = History::default();
    let mut previous: Option<Sample> = None;
    let mut rows = Vec::new();
    for (second, requests) in [
        (0, vec![listed("long-a", 1_150_000, Some(9.0), 0)]),
        (1, Vec::new()),
        (2, vec![listed("long-b", 1_200_000, Some(9.0), 2)]),
        (3, Vec::new()),
        (4, Vec::new()),
        (5, Vec::new()),
        (6, Vec::new()),
    ] {
        let mut sample = Sample {
            llm_provider: "oMLX".into(),
            llm_model: MODEL.into(),
            llm_observed_at: Some(at(second)),
            updated: "23:59:59".into(),
            llm_requests: requests,
            llm_prefill_tps: Some(100.0),
            ..Sample::default()
        };
        for request in &mut sample.llm_requests {
            request.cached = Some(if request.id == "long-a" { 1_150_000 } else { 0 });
        }
        if second == 6 {
            sample.llm_requests = vec![RequestUsage {
                completed: true,
                ..listed("hour", 900, Some(9.0), 6)
            }];
            sample.llm_request_timings = vec![RequestTiming {
                provider: "oMLX".into(),
                model: MODEL.into(),
                id: "hour".into(),
                prefill_tps: None,
                load_ms: Some(3_600_000),
            }];
        }
        history.observe(&sample.llm_requests);
        rows.extend(findings.observe(previous.as_ref(), &sample, &history));
        previous = Some(sample);
    }
    assert_eq!(rows.len(), 2, "{rows:?}");
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    for (state, summary) in rows.into_iter().chain([
        ("KV CACHE FULL", "1000 requests were recomputed".to_owned()),
        (
            "OVERLAP",
            ROWS[0].1.replace("0.97×", "1.09×").replace("3.0×", "11.5×"),
        ),
    ]) {
        app.collector.signals.push_back(SignalEvent {
            time: "23:59:59".into(),
            recorded_at: SystemTime::now(),
            kind: EventKind::from_state(state),
            state: state.into(),
            summary,
            tone: Tone::Cyan,
        });
    }
    app.collector.current.request_findings = Evidence {
        overlap: Some(Overlap {
            payoff: 1.09,
            seconds: 7_260.0,
            pairs: 1_204,
            beside: 125.5,
            alone: 10.9,
            beside_requests: 4_096,
            alone_requests: 4_096,
            extra_wait: 470_000.0,
        }),
        limit: Some(Limit {
            provider: "oMLX".into(),
            setting: "max_concurrent_requests",
            configured: 1,
            observed: 64,
        }),
        recomputed: 1_000,
        last_recomputed: Some("23:59:59".into()),
        ..findings.evidence()
    };
    app.collector.session.request_findings = findings;
    open(app, view)
}

/// Each case, with the text it must show whole and once at sizes that have
/// room for it (`from` columns and rows up).
fn cases() -> Vec<(Case, (u16, u16), Vec<&'static str>)> {
    let case = |name, app| Case {
        name,
        impact: false,
        app,
    };
    vec![
        (
            // The Overview's recent Journal shows from 120×40.
            case("request-findings-overview", with_findings(' ')),
            (120, 40),
            vec!["KV CACHE FULL", "MODEL LOAD"],
        ),
        (
            case("request-findings-journal", with_findings('j')),
            (200, 60),
            ROWS.iter().flat_map(|(state, summary)| [*state, *summary]).collect(),
        ),
        (
            case("request-findings-diagnostics", with_findings('d')),
            (200, 60),
            vec![
                "Cache misses: 1 long prompt read in full right after cached ones, ≈2m 56s of prompt reading",
                "Model loads: first tokens waited 6.5s for 1 load",
                "Two requests at once: 0.97× the work of one at a time over 3m 20s (10 pairs)",
                "oMLX is set to run 1 request at a time (max_concurrent_requests), yet 2 ran at once",
                "KV cache full: 3 requests recomputed this session",
            ],
        ),
        (
            case("request-findings-summary", with_findings('u')),
            (80, 24),
            vec![
                "Time lost before first tokens: 3m 43s",
                "cache misses               2m 56s",
                "model loads                7s",
                "overlap                    40s",
            ],
        ),
        (
            case("request-findings-widest-journal", widest('j')),
            (200, 60),
            vec![
                "long-b · 1.2M prompt read in full right after one of 1.1M was 100% cached · ≈3h 20m of prompt reading",
                "hour · first token waited 1h 0m for the model to load",
                "1000 requests were recomputed",
            ],
        ),
        (
            case("request-findings-widest-diagnostics", widest('d')),
            (200, 60),
            vec![
                "Two requests at once: 1.09× the work of one at a time over 2h 1m (1204 pairs)",
                "oMLX is set to run 1 request at a time (max_concurrent_requests), yet 64 ran at once",
                "KV cache full: 1000 requests recomputed this session",
            ],
        ),
        (
            case("request-findings-widest-summary", widest('u')),
            (80, 24),
            vec!["Time lost before first tokens: 4h 20m"],
        ),
        (case("request-findings-widest", widest(' ')), (80, 24), vec![]),
    ]
}

#[test]
fn request_findings_pass_the_qa_judge_at_every_size() {
    let mut failures = Vec::new();
    for (case, (wide, tall), lines) in cases() {
        for (width, height) in SIZES {
            for problem in judge(&case, width, height) {
                failures.push(format!("{} at {width}×{height}: {problem}", case.name));
            }
            if width < wide || height < tall {
                continue;
            }
            let screen = render_app(&case.app, width, height);
            for line in &lines {
                if screen.matches(line).count() != 1 {
                    failures.push(format!(
                        "{} at {width}×{height}: {line:?} not shown once and whole\n{screen}",
                        case.name
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "QA judge:\n{}", failures.join("\n"));
}

/// What only a finding may put on screen.
const FINDINGS: [&str; 9] = [
    "CACHE MISS",
    "MODEL LOAD",
    "OVERLAP",
    "KV CACHE FULL",
    "Cache misses",
    "Model loads",
    "Two requests at once",
    "ran at once",
    "Time lost before first tokens",
];

#[test]
fn a_session_with_nothing_lost_shows_nothing_of_it() {
    let mut failures = Vec::new();
    for view in [' ', 'd', 'u', 'j'] {
        let mut app = test_app(0);
        populate_dashboard_fixture(&mut app);
        let case = Case {
            name: "request-findings-quiet",
            impact: false,
            app: open(app, view),
        };
        for (width, height) in SIZES {
            for problem in judge(&case, width, height) {
                failures.push(format!("{view} at {width}×{height}: {problem}"));
            }
            let screen = render_app(&case.app, width, height);
            for finding in FINDINGS {
                if screen.contains(finding) {
                    failures.push(format!("{view} at {width}×{height}: shows {finding:?}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "QA judge:\n{}", failures.join("\n"));
}

#[test]
fn findings_reach_the_journal_diagnostics_and_summary_through_the_collector() {
    let collector = session();
    let told: Vec<(&str, &str)> = collector
        .signals
        .iter()
        .filter(|row| ROWS.iter().any(|(state, _)| row.state == *state))
        .map(|row| (row.state.as_str(), row.summary.as_str()))
        .collect();
    assert_eq!(told, ROWS);
    // Rows about requests, filed with the model's, in no impact colour.
    for row in collector
        .signals
        .iter()
        .filter(|row| ROWS.iter().any(|(state, _)| row.state == *state))
    {
        assert_eq!(row.kind, EventKind::Llm, "{}", row.state);
        assert_eq!(row.tone, Tone::Cyan, "{}", row.state);
    }
    // The evidence travels with the sample, for Diagnostics.
    let evidence = &collector.current.request_findings;
    assert_eq!(evidence.full_reads, 1);
    assert_eq!(
        evidence.last_full_read,
        Some(FullRead {
            at: "14:06:57".into(),
            prompt: 45_000,
            previous: 44_000,
            previous_percent: 98,
        })
    );
    let diagnostics = crate::report::diagnostic_lines(&collector.current, None);
    for line in crate::request_findings::lines(evidence) {
        assert!(
            diagnostics.contains(&line),
            "{line} not in {diagnostics:#?}"
        );
    }
    // The summary counts what first tokens lost.
    let summary = crate::session::lines(
        &collector.session,
        &collector.request_history,
        &collector.current,
        None,
    );
    let at = summary
        .iter()
        .position(|line| line == "Time lost before first tokens: 3m 43s")
        .unwrap_or_else(|| panic!("{summary:#?}"));
    assert_eq!(
        summary[at + 1..at + 4],
        [
            "  cache misses               2m 56s",
            "  model loads                7s",
            "  overlap                    40s",
        ]
    );
    // A reset starts the session over.
    let mut collector = collector;
    collector.reset();
    assert_eq!(collector.session.request_findings.lost()[0].1, 0.0);
}
