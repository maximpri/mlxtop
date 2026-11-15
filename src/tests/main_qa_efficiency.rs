// SPDX-License-Identifier: MIT
//! The QA judge on what decode efficiency adds to the screens: the note
//! under a healthy verdict, Diagnostics' share and the draft's payoff, and
//! the session summary's decode by context. Each at every size, with a
//! draft and without, with the widest figures, and with nothing to show.
use super::qa::{judge, Case, SIZES};
use super::*;
use crate::model_facts::ModelFacts;

/// The dashboard fixture's Mac Studio with its model's facts known, its
/// runtime counting 2.6 tokens a pass, changed by `change`, and a session
/// that has seen it decode thirty times.
fn decoding(change: fn(&mut Sample)) -> App {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    let sample = &mut app.collector.current;
    sample.model = ModelFacts {
        bytes_per_pass: Some(15_020_479_488),
        kv_layers: Some(16),
        kv_heads: Some(4),
        head_dim: Some(256),
        kv_bytes_per_element: Some(1.0 + 1.0 / 64.0),
        fixed_state_bytes: Some(160_038_912),
        draft: Some(true),
        ..ModelFacts::default()
    };
    sample.llm_tokens_per_pass = Some(2.6);
    change(sample);
    let sample = sample.clone();
    for _ in 0..30 {
        app.collector.session.observe(&sample);
    }
    app
}

/// A model that decodes without a draft, at 13 tok/s.
fn plain(sample: &mut Sample) {
    sample.model.draft = Some(false);
    sample.llm_tokens_per_pass = None;
    sample.llm_generation_tps = Some(13.0);
}

/// The widest figures: an M5 Ultra's 1200 GB/s nearly all used by a 100 GB
/// model 300k tokens deep, 9.9 tokens a pass, 8.6 tok/s without a draft.
fn widest(sample: &mut Sample) {
    sample.metal.device_name = Some("Apple M5 Ultra".into());
    sample.metal.gpu_cores = Some(80);
    sample.model.bytes_per_pass = Some(100_000_000_000);
    sample.model.kv_bytes_per_element = Some(4.0);
    sample.llm_tokens_per_pass = Some(9.9);
    sample.llm_requests[0].prompt = 270_000;
    sample.llm_requests[0].output = Some(30_000);
    let bytes = 100e9 + 160_038_912.0 + 300_000.0 * 131_072.0;
    sample.llm_generation_tps = Some(9.9 * 1200e9 / bytes * 0.999);
}

/// A draft whose runtime does not say how many tokens a pass emits.
fn uncounted(sample: &mut Sample) {
    sample.llm_tokens_per_pass = None;
}

/// A draft counted as one token a pass: 141% of the peak, an input wrong.
fn over_the_peak(sample: &mut Sample) {
    sample.llm_tokens_per_pass = Some(1.0);
}

/// `app` with Diagnostics (`d`) or the session summary (`u`) open.
fn open(mut app: App, view: char) -> App {
    match view {
        'd' => app.diagnostics_open = true,
        'u' => app.summary_open = true,
        _ => {}
    }
    app
}

const NOTE: &str = "Decode is near its estimated limit: fewer bytes/token or a better draft.";
const PLAIN_NOTE: &str = "Decode is near its estimated limit: fewer bytes/token or a draft model.";

/// Each case, with the lines it must show once and whole at every size.
fn cases() -> Vec<(Case, Vec<&'static str>)> {
    let case = |name, app| Case {
        name,
        impact: false,
        app,
    };
    vec![
        (case("efficiency-note", decoding(|_| {})), vec![NOTE]),
        (
            case("efficiency-diagnostics", open(decoding(|_| {}), 'd')),
            vec![
                "Note: Decode is near its estimated limit: fewer bytes/token or a better draft.",
                "Decode estimate: 54% of 410 GB/s per verify pass, draft excluded; best 54%.",
                "Draft: 2.6 tokens per pass; without it, at most an estimated 25 tok/s.",
            ],
        ),
        (
            case("efficiency-summary", open(decoding(|_| {}), 'u')),
            vec![
                "Decode per verify pass by context (estimate, draft excluded):",
                "16k–64k   54% of 410 GB/s · usual 36 tok/s · 2.6 tokens per pass",
            ],
        ),
        (
            case("efficiency-plain-note", decoding(plain)),
            vec![PLAIN_NOTE],
        ),
        (
            case("efficiency-plain-diagnostics", open(decoding(plain), 'd')),
            vec!["Decode estimate: 52% of 410 GB/s; best 52%."],
        ),
        (
            case("efficiency-plain-summary", open(decoding(plain), 'u')),
            vec![
                "Decode by context (estimate):",
                "16k–64k   52% of 410 GB/s · usual 13 tok/s",
            ],
        ),
        (case("efficiency-widest-note", decoding(widest)), vec![NOTE]),
        (
            case("efficiency-widest-diagnostics", open(decoding(widest), 'd')),
            vec![
                "Decode estimate: 100% of 1200 GB/s per verify pass, draft excluded; best 100%.",
                "Draft: 9.9 tokens per pass; without it, at most an estimated 8.6 tok/s.",
            ],
        ),
        (
            case("efficiency-widest-summary", open(decoding(widest), 'u')),
            vec!["256k+     100% of 1200 GB/s · usual 85 tok/s · 9.9 tokens per pass"],
        ),
    ]
}

/// Every share the screen gives of something: the whole number before `% of`.
fn shares(screen: &str) -> Vec<u32> {
    screen
        .match_indices("% of ")
        .filter_map(|(at, _)| {
            let digits: Vec<char> = screen[..at]
                .chars()
                .rev()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.into_iter().rev().collect::<String>().parse().ok()
        })
        .collect()
}

/// What only a known estimate may put on screen.
const ESTIMATES: [&str; 5] = [
    "Decode estimate",
    "estimated limit",
    "Decode per verify pass",
    "Decode by context",
    "Draft: ",
];

#[test]
fn decode_efficiency_passes_the_qa_judge_at_every_size() {
    let mut failures = Vec::new();
    for (case, lines) in cases() {
        for (width, height) in SIZES {
            for problem in judge(&case, width, height) {
                failures.push(format!("{} at {width}×{height}: {problem}", case.name));
            }
            // Each line once, on one row, never cut: a line that ran past
            // the panel would end mid-word or with `…` on that row.
            let screen = render_app(&case.app, width, height);
            for line in &lines {
                if screen.matches(line).count() != 1 {
                    failures.push(format!(
                        "{} at {width}×{height}: {line:?} not shown once and whole\n{screen}",
                        case.name
                    ));
                }
            }
            // A share is never above the peak.
            if let Some(share) = shares(&screen).into_iter().find(|share| *share > 100) {
                failures.push(format!("{}: a share of {share}%", case.name));
            }
        }
    }
    assert!(failures.is_empty(), "QA judge:\n{}", failures.join("\n"));
}

#[test]
fn nothing_is_shown_without_a_trustworthy_estimate() {
    let mut failures = Vec::new();
    for (name, change) in [
        ("efficiency-uncounted", uncounted as fn(&mut Sample)),
        ("efficiency-over-the-peak", over_the_peak),
    ] {
        for view in [' ', 'd', 'u'] {
            let case = Case {
                name,
                impact: false,
                app: open(decoding(change), view),
            };
            for (width, height) in SIZES {
                for problem in judge(&case, width, height) {
                    failures.push(format!("{name} {view} at {width}×{height}: {problem}"));
                }
                let screen = render_app(&case.app, width, height);
                for estimate in ESTIMATES {
                    if screen.contains(estimate) {
                        failures.push(format!(
                            "{name} {view} at {width}×{height}: shows {estimate:?}\n{screen}"
                        ));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "QA judge:\n{}", failures.join("\n"));
}
