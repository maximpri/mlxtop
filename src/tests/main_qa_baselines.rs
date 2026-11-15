// SPDX-License-Identifier: MIT
//! The QA judge on what usual speeds kept across sessions add to the
//! screens: the finding against earlier sessions on the strip, in
//! Diagnostics and on the generation chart; the reset prompt with and
//! without kept speeds; the Settings key; and the Journal's notice. Each at
//! every size.
use super::qa::{judge, Case, SIZES};
use super::*;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use std::fs;
use std::time::SystemTime;

const SENTENCE: &str = "This model now generates 31% below its usual from earlier sessions at this depth, since oMLX 0.7.3 became 0.8.0";

/// The dashboard fixture slowed `cause`, as the engine leaves a slowdown:
/// its insight, and its samples marked on the generation chart.
fn slowed(cause: CorrelationCause, lead: &str, earlier: &str) -> App {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    let reasons = if cause == CorrelationCause::EarlierSessions {
        Vec::new()
    } else {
        vec![cause]
    };
    app.collector.current.correlation = CorrelationInsight {
        direction: ThroughputDirection::Down,
        cause,
        lead: lead.into(),
        summary: format!("GEN ↓31.0% (58.0→40.0 tok/s) · {lead}"),
        delta_percent: Some(-31.0),
        slow: true,
        unusual: reasons.clone(),
        notes: reasons.iter().map(|_| lead.to_owned()).collect(),
        episode: Some(domain::DropEpisode {
            reference: 58.0,
            low: 39.5,
            seconds: 6.0,
            samples: 6,
        }),
        usual_speed: Some((58.0, 46.4)),
        usual_for: "1 request · 33.9k context".into(),
        earlier: earlier.into(),
        ..CorrelationInsight::default()
    };
    for point in app.collector.generation_history.iter_mut().rev().take(3) {
        if point.value.is_some() {
            point.tone = Tone::Unusual;
        }
    }
    app
}

fn below_earlier() -> App {
    slowed(
        CorrelationCause::EarlierSessions,
        "below earlier sessions",
        SENTENCE,
    )
}

/// The first judged sample below earlier sessions: a note, not painted.
fn first_below() -> App {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    app.collector.current.correlation = CorrelationInsight {
        direction: ThroughputDirection::Flat,
        slow: true,
        delta_percent: Some(-31.0),
        unusual: vec![CorrelationCause::EarlierSessions],
        notes: vec![SENTENCE.into()],
        usual_speed: Some((58.0, 46.4)),
        usual_for: "1 request · 33.9k context".into(),
        earlier: SENTENCE.into(),
        ..CorrelationInsight::default()
    };
    app
}

fn reset_prompt(baselines: bool) -> App {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    app.baselines = baselines;
    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    app
}

fn settings() -> App {
    let mut app = test_app(3);
    populate_dashboard_fixture(&mut app);
    app.tab = 3;
    app.settings.select("baselines");
    app
}

const NOTICE: &str = "usual speeds now kept in ~/Library/Application Support/mlxtop/baselines.json · Settings (4) turns this off";

/// The Journal with the notice the first save leaves, on `tab`.
fn noticed(tab: usize) -> App {
    let mut app = test_app(tab);
    populate_dashboard_fixture(&mut app);
    app.tab = tab;
    app.collector.signals.push_back(SignalEvent {
        time: "12:00:01".into(),
        recorded_at: SystemTime::now(),
        kind: EventKind::System,
        state: "SYSTEM".into(),
        summary: NOTICE.into(),
        tone: Tone::Cyan,
    });
    app
}

fn open(mut app: App) -> App {
    app.diagnostics_open = true;
    app
}

fn cases() -> Vec<Case> {
    let case = |name, impact, app| Case { name, impact, app };
    vec![
        case("earlier-sessions-slowdown", true, below_earlier()),
        case("earlier-sessions-diagnostics", true, open(below_earlier())),
        case("earlier-sessions-note", false, first_below()),
        case(
            "earlier-sessions-note-diagnostics",
            false,
            open(first_below()),
        ),
        case("reset-prompt", false, reset_prompt(true)),
        case("reset-prompt-without-baselines", false, reset_prompt(false)),
        case("settings-baselines", false, settings()),
        case("baselines-notice-journal", false, noticed(2)),
        case("baselines-notice-overview", false, noticed(0)),
    ]
}

/// Whether every row of `screen` that shows the start of `text` shows it
/// whole, or up to a word: at the row's end where it wraps, or marked `…`
/// where it was shortened.
fn cut_at_a_word(screen: &str, text: &str) -> bool {
    let start = text
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ");
    screen.lines().all(|row| {
        let Some(at) = row.find(&start) else {
            return true;
        };
        // What the row shows of it, up to a panel's edge.
        let shown = row[at..].split('│').next().unwrap_or("").trim_end();
        if shown.starts_with(text) {
            return true;
        }
        let kept = shown.strip_suffix('…').unwrap_or(shown).trim_end();
        text.starts_with(kept) && text[kept.len()..].starts_with(' ')
    })
}

#[test]
fn what_kept_speeds_add_passes_the_qa_judge_at_every_size() {
    let mut failures = Vec::new();
    for case in cases() {
        for (width, height) in SIZES {
            for problem in judge(&case, width, height) {
                failures.push(format!("{} at {width}×{height}: {problem}", case.name));
            }
            let screen = render_app(&case.app, width, height);
            // The sentence, wherever it shows, is cut only at a word.
            for text in [SENTENCE, NOTICE] {
                if !cut_at_a_word(&screen, text) {
                    failures.push(format!(
                        "{} at {width}×{height}: {text:?} cut mid-word\n{screen}",
                        case.name
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "QA judge:\n{}", failures.join("\n"));
}

#[test]
fn the_finding_against_earlier_sessions_reads_where_a_slowdown_does() {
    // The strip: the verdict and the sentence as its next step, cut at a
    // word where it does not fit.
    let narrow = render_app(&below_earlier(), 80, 24);
    assert!(
        narrow.contains("✖ Generation slowed · GEN ↓31.0% (58.0→40.0 tok/s) for 6s"),
        "{narrow}"
    );
    assert!(
        narrow.contains("→ This model now generates 31% below its usual from"),
        "{narrow}"
    );
    let wide = render_app(&below_earlier(), 200, 60);
    assert!(wide.contains(&format!("→ {SENTENCE}.")), "{wide}");
    // The generation chart names it under its bars where they have room,
    // as every reason does; at 80 columns only `▼` fits there.
    let medium = render_app(&below_earlier(), 120, 40);
    assert!(medium.contains("▼ below earlier sessions"), "{medium}");
    // Diagnostics: what it is compared with, and the sentence whole.
    let diagnostics = render_app(&open(below_earlier()), 200, 60);
    for line in [
        "Usual from earlier sessions",
        &format!("Check: {SENTENCE}."),
    ] {
        assert!(diagnostics.contains(line), "{line}\n{diagnostics}");
    }
    // Its first judged sample: said in Diagnostics, nothing painted.
    let note = render_app(&open(first_below()), 200, 60);
    assert!(note.contains(&format!("Unusual: {SENTENCE}")), "{note}");
    assert!(render_app(&first_below(), 120, 40).contains("✓ Healthy"));
}

#[test]
fn reset_asks_first_and_offers_to_forget_kept_speeds() {
    let prompt = render_app(&reset_prompt(true), 80, 24);
    for line in [
        "RESET?",
        "Clears charts, the Journal and session totals.",
        "y reset · any other key keeps everything",
        "f also forgets usual speeds kept across sessions",
    ] {
        assert!(prompt.contains(line), "{line}\n{prompt}");
    }
    let plain = render_app(&reset_prompt(false), 80, 24);
    assert!(!plain.contains("f also forgets"), "{plain}");
    // y resets, f also forgets, anything else keeps everything.
    let answered = |key: char, baselines: bool| {
        let (mut app, _views) = test_app_with_sender(0);
        let (commands, received) = std::sync::mpsc::channel();
        app.sampler.commands = commands;
        app.baselines = baselines;
        app.alert = Some(ActiveAlert {
            state: "HEAVY PAGING".into(),
            summary: "swap".into(),
            time: "now".into(),
        });
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert!(app.alert.is_some(), "nothing goes before the answer");
        app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
        assert!(!app.reset_prompt);
        let sent: Vec<SamplerCommand> = received.try_iter().collect();
        (app.alert.is_none(), sent)
    };
    assert!(
        matches!(answered('y', true), (true, sent) if matches!(sent[..], [SamplerCommand::Reset]))
    );
    assert!(
        matches!(answered('f', true), (true, sent) if matches!(sent[..], [SamplerCommand::Forget]))
    );
    assert!(matches!(answered('f', false), (false, sent) if sent.is_empty()));
    assert!(matches!(answered('n', true), (false, sent) if sent.is_empty()));
    // Ctrl-C at the prompt still quits; Enter is yes.
    let mut app = reset_prompt(true);
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(app.quit);
    let mut app = reset_prompt(true);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.reset_prompt && !app.quit);
    // Too small a terminal still shows the question it is answering.
    let mut app = reset_prompt(true);
    render_app(&app, 60, 15);
    assert!(render_app(&app, 60, 15).contains("RESET?"));
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert!(!app.reset_prompt);
    // While it asks, the mouse does not reach the charts underneath.
    let click = |app: &mut App| {
        render_app(app, 120, 40);
        let (_, region) = app.charts.regions.borrow()[0];
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: region.x + 2,
            row: region.y + 1,
            modifiers: KeyModifiers::NONE,
        });
        app.charts.expanded
    };
    assert!(!click(&mut reset_prompt(true)));
    let mut unasked = reset_prompt(true);
    unasked.reset_prompt = false;
    assert!(click(&mut unasked), "the click lands on a chart");
}

#[test]
fn settings_turns_kept_speeds_off_and_says_where_they_are() {
    let screen = render_app(&settings(), 120, 40);
    assert!(screen.contains("Keep usual speeds"), "{screen}");
    assert!(screen.contains("baselines.json"), "{screen}");
    let dir = crate::test_support::TempDir::new("settings-baselines");
    let mut app = settings();
    app.settings.path = dir.0.join("config.json");
    app.settings.reload();
    app.settings.select("baselines");
    let (commands, received) = std::sync::mpsc::channel();
    app.sampler.commands = commands;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        !app.baselines,
        "the reset prompt no longer offers to forget"
    );
    let config: Config =
        serde_json::from_str(&fs::read_to_string(dir.0.join("config.json")).unwrap()).unwrap();
    assert_eq!(config.baselines, Some(false));
    assert!(received
        .try_iter()
        .any(|command| matches!(command, SamplerCommand::SetConfig(config) if config.baselines == Some(false))));
}

/// Writes this file's screens as text for a person or `scripts/qa_judge.py`
/// to read, as the main judge's dump does.
#[test]
#[ignore = "run by hand with QA_SCREENS_DIR set"]
fn qa_screens_of_kept_speeds() {
    let Some(dir) = std::env::var_os("QA_SCREENS_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    fs::create_dir_all(&dir).unwrap();
    for case in cases() {
        for (width, height) in SIZES {
            let screen = render_app(&case.app, width, height);
            fs::write(
                dir.join(format!("{}-{width}x{height}.txt", case.name)),
                screen,
            )
            .unwrap();
        }
    }
}
