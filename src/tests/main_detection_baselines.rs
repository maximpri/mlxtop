// SPDX-License-Identifier: MIT
//! Usual speeds kept across sessions, as the engine judges with them: work
//! this session has not yet done is judged against earlier sessions' from
//! its first sample, a model that now runs below them says so in one
//! sentence, and only what generation held outside slowdowns is kept.
use super::*;
use crate::baseline_store::{read, write, Band, Baselines, Entry, Identity, Per, Read};
use crate::model_facts::ModelFacts;
use crate::test_support::TempDir;

/// A usual of 58 tok/s, as a window of earlier sessions' speeds.
const USUAL_58: [f64; 10] = [57.0, 58.0, 59.0, 57.5, 58.5, 58.0, 57.8, 58.2, 58.0, 58.1];

const SENTENCE: &str = "This model now generates 31% below its usual from earlier sessions at this depth, since oMLX 0.7.3 became 0.8.0";

/// A poll of request `id` writing alone at `tps`, `context` tokens deep,
/// served by oMLX `version`.
fn on(version: &str, id: &str, second: u64, tps: f64, context: u64) -> Sample {
    let mut sample = paging(second, tps, 0);
    let request = &mut sample.llm_requests[0];
    request.id = id.into();
    request.prompt = context - 200;
    request.output = Some(200);
    sample.model = ModelFacts {
        quantization: Some("4-bit".into()),
        runtime_version: Some(version.into()),
        ..ModelFacts::default()
    };
    sample
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// What the file keeps for a lone request 16k–64k deep on `version`.
fn entry(version: &str, per: Per, speeds: &[f64]) -> Entry {
    let identity = Identity::of(&on(version, "x", 0, 1.0, 30_000)).unwrap();
    Entry {
        machine: identity.machine,
        provider: identity.provider,
        model: identity.model,
        quantization: identity.quantization,
        runtime: identity.runtime,
        draft: identity.draft,
        context: Band::To64k,
        depth: Band::To64k.depth(),
        requests: 1,
        per,
        seen: now_seconds(),
        speeds: speeds.to_vec(),
    }
}

/// An engine keeping usual speeds in a file holding `entries`.
fn keeping(dir: &TempDir, entries: &[Entry]) -> (CorrelationEngine, PathBuf) {
    let path = dir.0.join("baselines.json");
    write(&path, entries).unwrap();
    let engine = CorrelationEngine::with_baselines(Baselines::new(path.clone(), true));
    (engine, path)
}

/// What the file now holds for `version`, once the engine saved.
fn kept(engine: &mut CorrelationEngine, path: &Path, version: &str) -> Vec<f64> {
    engine.baselines.as_deref_mut().unwrap().flush();
    let Read::Entries(entries) = read(path) else {
        panic!("readable");
    };
    entries
        .into_iter()
        .find(|entry| entry.runtime.as_deref() == Some(version))
        .map(|entry| entry.speeds)
        .unwrap_or_default()
}

/// What the file holds: each kind's runtime version and speeds.
fn on_file(path: &Path) -> Vec<(Option<String>, Vec<f64>)> {
    let Read::Entries(entries) = read(path) else {
        panic!("readable");
    };
    entries
        .into_iter()
        .map(|entry| (entry.runtime, entry.speeds))
        .collect()
}

/// The file as these tests write it, before anything is saved.
fn untouched() -> Vec<(Option<String>, Vec<f64>)> {
    vec![(Some("oMLX 0.7.3".into()), USUAL_58.to_vec())]
}

#[test]
fn a_model_that_now_runs_below_earlier_sessions_says_so_from_its_first_judged_sample() {
    let dir = TempDir::new("kept-58");
    let (mut engine, _) = keeping(&dir, &[entry("oMLX 0.7.3", Per::Token, &USUAL_58)]);
    // A new session after oMLX 0.7.3 became 0.8.0, at 40 tok/s 30k deep.
    let mut samples: Vec<Sample> = (0..8)
        .map(|second| on("oMLX 0.8.0", "a", second, 40.0, 30_000))
        .collect();
    observe_all(&mut engine, &mut samples);
    // The request's first poll is shown, never judged.
    assert!(samples[0].correlation.notes.is_empty());
    // The first judged sample: judged against earlier sessions at once,
    // and the sentence said, in Diagnostics; nothing painted yet.
    let first = &samples[1].correlation;
    assert_eq!(first.usual_speed.map(|(usual, _)| usual), Some(58.0));
    assert_eq!(first.usual_for, "1 request · 30.0k context");
    assert_eq!(first.notes, vec![SENTENCE]);
    assert_eq!(first.unusual, vec![CorrelationCause::EarlierSessions]);
    assert!(first.slow && !first.is_material_drop());
    // Held for three samples, it is a slowdown like any other.
    let slowed = &samples[3].correlation;
    assert!(slowed.is_material_drop(), "{}", slowed.summary);
    assert_eq!(slowed.cause, CorrelationCause::EarlierSessions);
    assert_eq!(
        slowed.summary,
        "GEN ↓31.0% (58.0→40.0 tok/s) · below earlier sessions"
    );
    assert_eq!(slowed.lead, "below earlier sessions");
    assert_eq!(slowed.earlier, SENTENCE);
    assert_eq!(slowed.tone(), Tone::Red, "31% is a severe drop");
    assert_eq!(slowed.marked_samples(), SLOW_SAMPLES);
    // Told once in the Journal, with the sentence as what to know.
    assert_eq!(
        journal_rows(&samples, &["SLOWDOWN"]),
        vec![(
            "SLOWDOWN".to_owned(),
            format!("GEN ↓31.0% (58.0→40.0 tok/s) · below earlier sessions → {SENTENCE}.")
        )]
    );
    // The verdict leads with it.
    let finding = crate::diagnosis::assess(&samples[3]);
    assert_eq!(finding.title, "Generation slowed");
    assert_eq!(finding.evidence, "GEN ↓31.0% (58.0→40.0 tok/s)");
    assert_eq!(finding.context, "Usual from earlier sessions");
    assert_eq!(finding.next, format!("{SENTENCE}."));
}

#[test]
fn the_same_speed_as_earlier_sessions_says_nothing_and_is_kept_as_the_new_versions() {
    let dir = TempDir::new("kept-equal");
    let (mut engine, path) = keeping(&dir, &[entry("oMLX 0.7.3", Per::Token, &USUAL_58)]);
    let mut samples: Vec<Sample> = (0..12)
        .map(|second| {
            on(
                "oMLX 0.8.0",
                "a",
                second,
                58.0 + (second % 3) as f64 * 0.5,
                30_000,
            )
        })
        .collect();
    observe_all(&mut engine, &mut samples);
    for sample in &samples {
        let insight = &sample.correlation;
        assert!(
            !insight.slow && !insight.is_material_drop() && insight.notes.is_empty(),
            "{}",
            insight.summary
        );
    }
    // Judged from the first judged sample, without learning eight first.
    assert_eq!(
        samples[1].correlation.usual_speed.map(|(usual, _)| usual),
        Some(58.0)
    );
    // What the new version ran at is kept as its own; the old stays.
    assert_eq!(kept(&mut engine, &path, "oMLX 0.8.0").len(), 11);
    assert_eq!(kept(&mut engine, &path, "oMLX 0.7.3"), USUAL_58.to_vec());
}

#[test]
fn below_earlier_sessions_on_the_same_version_names_no_update() {
    let dir = TempDir::new("kept-same");
    let (mut engine, _) = keeping(&dir, &[entry("oMLX 0.8.0", Per::Token, &USUAL_58)]);
    let mut samples: Vec<Sample> = (0..5)
        .map(|second| on("oMLX 0.8.0", "a", second, 40.0, 30_000))
        .collect();
    observe_all(&mut engine, &mut samples);
    let sentence =
        "This model now generates 31% below its usual from earlier sessions at this depth";
    assert_eq!(samples[1].correlation.notes, vec![sentence]);
    assert_eq!(samples[4].correlation.earlier, sentence);
    assert_eq!(
        crate::diagnosis::slowdown_next_step(&samples[4]),
        format!("{sentence}.")
    );
    let mut unworded = samples[4].clone();
    unworded.correlation.earlier.clear();
    assert_eq!(
        crate::diagnosis::slowdown_next_step(&unworded),
        "This model now generates below its usual from earlier sessions at this depth."
    );
    // Another kind of work has no usual from earlier sessions: learned.
    let mut deeper = vec![
        on("oMLX 0.8.0", "b", 100, 20.0, 90_000),
        on("oMLX 0.8.0", "b", 101, 20.0, 90_000),
    ];
    observe_all(&mut engine, &mut deeper);
    assert_eq!(deeper[1].correlation.usual_speed, None);
}

#[test]
fn nothing_learned_in_a_slowdown_or_by_the_settle_rule_is_kept() {
    let dir = TempDir::new("kept-episodes");
    let (mut engine, path) = keeping(&dir, &[entry("oMLX 0.8.0", Per::Token, &USUAL_58)]);
    // Usual speed, then a slowdown with paging beside it, then back.
    let mut samples: Vec<Sample> = (0..10)
        .map(|second| on("oMLX 0.8.0", "a", second, 58.0, 30_000))
        .collect();
    samples.extend((10..16).map(|second| {
        let mut sample = on("oMLX 0.8.0", "a", second, 30.0, 30_000);
        sample.swap_in = 96 * MIB;
        sample
    }));
    samples.extend((16..20).map(|second| on("oMLX 0.8.0", "a", second, 58.0, 30_000)));
    observe_all(&mut engine, &mut samples);
    assert_eq!(samples[12].correlation.cause, CorrelationCause::Paging);
    assert!(samples[18].correlation.ended.is_some(), "it recovered");
    // Nine usual samples before it, three that came back and one after:
    // never the slow ones.
    let speeds = kept(&mut engine, &path, "oMLX 0.8.0");
    assert_eq!(speeds.len(), USUAL_58.len() + 13);
    assert!(speeds.iter().all(|speed| *speed > 50.0), "{speeds:?}");

    // A slower speed that holds with nothing else unusual settles; what
    // the settle rule adopts is not kept, what comes after is.
    let dir = TempDir::new("kept-settle");
    let (mut engine, path) = keeping(&dir, &[entry("oMLX 0.8.0", Per::Token, &USUAL_58)]);
    let mut samples: Vec<Sample> = (0..45)
        .map(|second| on("oMLX 0.8.0", "a", second, 40.0, 30_000))
        .collect();
    observe_all(&mut engine, &mut samples);
    let settled = samples
        .iter()
        .position(|sample| sample.correlation.ended.is_some_and(|end| end.settled))
        .expect("it settles after 30 s");
    let speeds = kept(&mut engine, &path, "oMLX 0.8.0");
    assert_eq!(speeds.len(), USUAL_58.len() + (samples.len() - 1 - settled));
    assert!(speeds[USUAL_58.len()..].iter().all(|speed| *speed == 40.0));
}

#[test]
fn a_session_that_ends_below_earlier_sessions_is_judged_against_itself_after() {
    let dir = TempDir::new("kept-once");
    let (mut engine, path) = keeping(&dir, &[entry("oMLX 0.7.3", Per::Token, &USUAL_58)]);
    // One request below earlier sessions, ending while slow.
    let mut samples: Vec<Sample> = (0..6)
        .map(|second| on("oMLX 0.8.0", "a", second, 40.0, 30_000))
        .collect();
    // The next, a minute later: told no more, learned as this session's.
    samples.extend((70..82).map(|second| on("oMLX 0.8.0", "b", second, 40.0, 30_000)));
    observe_all(&mut engine, &mut samples);
    assert!(samples[3].correlation.is_material_drop());
    assert!(samples[6..].iter().all(|sample| {
        let insight = &sample.correlation;
        !insight.slow && insight.notes.is_empty() && !insight.is_material_drop()
    }));
    assert_eq!(
        journal_rows(&samples, &["SLOWDOWN", "ENDED"])
            .iter()
            .map(|(state, _)| state.as_str())
            .collect::<Vec<_>>(),
        vec!["SLOWDOWN", "ENDED"]
    );
    // What the next request ran at is the new version's usual.
    assert_eq!(kept(&mut engine, &path, "oMLX 0.8.0").len(), 11);
}

#[test]
fn below_earlier_sessions_is_told_once_a_session_even_when_speed_comes_back() {
    let dir = TempDir::new("kept-back");
    let (mut engine, _) = keeping(&dir, &[entry("oMLX 0.7.3", Per::Token, &USUAL_58)]);
    // Below earlier sessions, then back to their usual speed.
    let mut samples: Vec<Sample> = (0..6)
        .map(|second| on("oMLX 0.8.0", "a", second, 40.0, 30_000))
        .collect();
    samples.extend((6..10).map(|second| on("oMLX 0.8.0", "a", second, 58.0, 30_000)));
    // Later, below them again: this session's own, not told again.
    samples.extend((40..50).map(|second| on("oMLX 0.8.0", "b", second, 40.0, 30_000)));
    observe_all(&mut engine, &mut samples);
    assert!(samples[3].correlation.is_material_drop());
    assert!(samples[8]
        .correlation
        .ended
        .is_some_and(|end| !end.settled && !end.finished));
    assert!(samples[10..].iter().all(|sample| {
        let insight = &sample.correlation;
        insight.notes.is_empty() && insight.cause != CorrelationCause::EarlierSessions
    }));
    assert_eq!(journal_rows(&samples, &["SLOWDOWN"]).len(), 1, "told once");
}

#[test]
fn speeds_kept_per_verify_pass_are_taken_to_this_texts_tokens_and_this_depth() {
    // The owner's model: 15.02 GB a pass, 33,280 KV bytes a token.
    let facts = |version: &str| ModelFacts {
        bytes_per_pass: Some(15_020_479_488),
        kv_layers: Some(16),
        kv_heads: Some(4),
        head_dim: Some(256),
        kv_bytes_per_element: Some(1.0 + 1.0 / 64.0),
        quantization: Some("4-bit".into()),
        runtime_version: Some(version.into()),
        draft: Some(true),
        ..ModelFacts::default()
    };
    let poll = |second: u64, tps: f64, context: u64, tokens: f64| {
        let mut sample = on("Splash 1.6.1", "a", second, tps, context);
        sample.model = facts("Splash 1.6.1");
        sample.llm_tokens_per_pass = Some(tokens);
        sample
    };
    let identity = Identity::of(&poll(0, 1.0, 30_000, 7.0)).unwrap();
    let passes = Entry {
        machine: identity.machine.clone(),
        provider: identity.provider.clone(),
        model: identity.model.clone(),
        quantization: identity.quantization.clone(),
        runtime: identity.runtime.clone(),
        draft: identity.draft,
        per: Per::Pass,
        ..entry("x", Per::Pass, &[12.0; 10])
    };
    let dir = TempDir::new("kept-passes");
    let (mut engine, path) = keeping(&dir, &[passes]);
    // 12 passes a second 32k deep is, at 20k deep and 7 tokens a pass:
    let (weights, kv) = (15_020_479_488.0, 33_280.0);
    let usual = 12.0 * 7.0 * (weights + kv * 32_768.0) / (weights + kv * 20_000.0);
    let mut samples: Vec<Sample> = (0..4)
        .map(|second| poll(second, usual, 20_000, 7.0))
        .collect();
    observe_all(&mut engine, &mut samples);
    let (judged, _) = samples[1].correlation.usual_speed.unwrap();
    assert!((judged - usual).abs() < 0.01, "{judged} vs {usual}");
    assert!(!samples[3].correlation.slow);
    // And kept as it came: 12 passes a second at the band's depth.
    let speeds = engine.baselines.as_deref_mut().map(|baselines| {
        baselines.flush();
        let Read::Entries(entries) = read(&path) else {
            panic!("readable");
        };
        entries[0].speeds.clone()
    });
    assert!(speeds.unwrap()[10..]
        .iter()
        .all(|speed| (*speed - 12.0).abs() < 0.01));
    // Speeds kept per pass judge nothing the runtime does not count.
    let mut uncounted = vec![poll(10, 30.0, 20_000, 7.0), poll(11, 30.0, 20_000, 7.0)];
    for sample in &mut uncounted {
        sample.llm_tokens_per_pass = None;
        sample.llm_requests[0].id = "c".into();
    }
    observe_all(&mut engine, &mut uncounted);
    assert_eq!(uncounted[1].correlation.usual_speed, None);
}

#[test]
fn a_runtime_updated_while_watched_is_judged_against_its_last_version() {
    let dir = TempDir::new("kept-update");
    let (mut engine, path) = keeping(&dir, &[entry("oMLX 0.7.3", Per::Token, &USUAL_58)]);
    let mut samples: Vec<Sample> = (0..5)
        .map(|second| on("oMLX 0.7.3", "a", second, 58.0, 30_000))
        .collect();
    // oMLX restarts as 0.8.0 under the same model name.
    samples.extend((5..10).map(|second| on("oMLX 0.8.0", "b", second, 40.0, 30_000)));
    observe_all(&mut engine, &mut samples);
    assert_eq!(samples[6].correlation.notes, vec![SENTENCE]);
    // What 0.7.3 ran at this session was saved as its own.
    assert_eq!(
        kept(&mut engine, &path, "oMLX 0.7.3").len(),
        USUAL_58.len() + 4
    );
}

#[test]
fn kept_speeds_off_or_for_another_model_judge_as_a_fresh_session_does() {
    let dir = TempDir::new("kept-off");
    let path = dir.0.join("baselines.json");
    write(&path, &[entry("oMLX 0.7.3", Per::Token, &USUAL_58)]).unwrap();
    let samples: Vec<Sample> = (0..40)
        .map(|second| {
            on(
                "oMLX 0.8.0",
                "a",
                second,
                if second < 20 { 58.0 } else { 30.0 },
                30_000,
            )
        })
        .collect();
    let judged = |mut engine: CorrelationEngine| {
        let mut samples = samples.clone();
        observe_all(&mut engine, &mut samples);
        samples
            .iter()
            .map(|sample| sample.correlation.summary.clone())
            .collect::<Vec<_>>()
    };
    let fresh = judged(CorrelationEngine::default());
    assert_eq!(
        judged(CorrelationEngine::with_baselines(Baselines::new(
            path.clone(),
            false
        ))),
        fresh
    );
    // A file with nothing for this model changes nothing.
    let other = dir.0.join("other.json");
    write(&other, &[]).unwrap();
    assert_eq!(
        judged(CorrelationEngine::with_baselines(Baselines::new(
            other, true
        ))),
        fresh
    );
    // Off, nothing is written either.
    assert_eq!(on_file(&path), untouched());
    // With a draft whose passes are not counted, tokens a second are also
    // the text: another session's are not judged against.
    let drafting = dir.0.join("drafting.json");
    let mut kept = entry("oMLX 0.8.0", Per::Token, &USUAL_58);
    kept.draft = Some(true);
    write(&drafting, &[kept]).unwrap();
    let mut samples = samples.clone();
    for sample in &mut samples {
        sample.model.draft = Some(true);
    }
    let mut engine = CorrelationEngine::with_baselines(Baselines::new(drafting, true));
    let mut fresh_samples = samples.clone();
    observe_all(&mut engine, &mut samples);
    observe_all(&mut CorrelationEngine::default(), &mut fresh_samples);
    let summaries = |samples: &[Sample]| {
        samples
            .iter()
            .map(|sample| sample.correlation.summary.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(summaries(&samples), summaries(&fresh_samples));
}

#[test]
fn a_replay_judges_against_a_baseline_file_only_when_given_one() {
    let dir = TempDir::new("kept-replay");
    let path = dir.0.join("baselines.json");
    write(&path, &[entry("oMLX 0.7.3", Per::Token, &USUAL_58)]).unwrap();
    let recording: Vec<Sample> = (0..8)
        .map(|second| on("oMLX 0.8.0", "a", second, 40.0, 30_000))
        .collect();
    // Without one, a fresh engine: 40 tok/s is simply this session's usual.
    let mut samples = recording.clone();
    let fresh = crate::recording::replay(&mut samples, defaults());
    assert!(fresh.tally.is_empty());
    // With one, the update's slowdown is counted under its own reason.
    let mut samples = recording.clone();
    let mut engine = CorrelationEngine::with_baselines(Baselines::read_only(&path).unwrap());
    let kept = crate::recording::replay_with(&mut samples, defaults(), &mut engine);
    assert_eq!(kept.tally.len(), 1);
    let (cause, tally) = kept.tally[0];
    assert_eq!(cause.label(), "below earlier sessions");
    assert_eq!(tally.episodes, 1);
    assert_eq!(tally.samples, 5);
    // The file is read, never written.
    assert_eq!(on_file(&path), untouched());
    // `mlxtop replay FILE --baselines KEPT` says it in its report.
    let file = dir.0.join("recording.jsonl");
    let lines: Vec<String> = recording
        .iter()
        .map(|sample| serde_json::to_string(sample).unwrap())
        .collect();
    fs::write(&file, lines.join("\n")).unwrap();
    let mut out = Vec::new();
    crate::recording::run(&file, defaults(), Some(&path), &mut out).unwrap();
    let report = String::from_utf8(out).unwrap();
    let row = report
        .lines()
        .find(|line| line.starts_with("below earlier sessions"))
        .expect("the reason is tallied");
    assert_eq!(
        row.split_whitespace().skip(3).take(2).collect::<Vec<_>>(),
        vec!["5", "1"],
        "{report}"
    );
    assert!(report.contains(SENTENCE), "{report}");
    assert!(crate::recording::run(
        &file,
        defaults(),
        Some(&dir.0.join("none.json")),
        &mut Vec::new()
    )
    .is_err());
}

/// Kept speeds over a recorded corpus, as if mlxtop restarted every
/// `MLXTOP_CORPUS_HOURS` (24 by default): each session replayed by a fresh
/// engine and by one keeping the earlier sessions' usual speeds in a file,
/// with the reasons each gave and every finding against earlier sessions.
/// Run by hand:
/// `MLXTOP_CORPUS=FILE cargo test --release -- --ignored kept_speeds_over_a_corpus --nocapture`
#[test]
#[ignore = "run by hand on a recorded corpus"]
fn kept_speeds_over_a_corpus() {
    let Some(path) = std::env::var_os("MLXTOP_CORPUS") else {
        return;
    };
    let hours: u64 = std::env::var("MLXTOP_CORPUS_HOURS")
        .ok()
        .and_then(|hours| hours.parse().ok())
        .unwrap_or(24);
    let file = std::fs::File::open(path).unwrap();
    let (samples, _) = crate::recording::read_counted(std::io::BufReader::new(file)).unwrap();
    let span = |sample: &Sample| sample.ts.map(|ms| ms / (hours * 3_600_000));
    let mut sessions: Vec<Vec<Sample>> = Vec::new();
    for sample in samples {
        match sessions.last_mut() {
            Some(last) if span(&last[0]) == span(&sample) => last.push(sample),
            _ => sessions.push(vec![sample]),
        }
    }
    let dir = TempDir::new("corpus-kept");
    let kept_in = dir.0.join("baselines.json");
    let mut totals: std::collections::BTreeMap<String, [usize; 4]> = Default::default();
    let mut generating = 0;
    let mut hours_generating = 0.0;
    println!("{} sessions of {hours} h", sessions.len());
    for session in sessions {
        let mut fresh = session.clone();
        let fresh = crate::recording::replay(&mut fresh, defaults());
        let mut kept = session;
        let mut engine = CorrelationEngine::with_baselines(Baselines::new(kept_in.clone(), true));
        let kept_tally = crate::recording::replay_with(&mut kept, defaults(), &mut engine);
        engine.baselines.as_deref_mut().unwrap().flush();
        generating += fresh.generating;
        hours_generating += fresh.generating_seconds / 3600.0;
        for (index, replay) in [&fresh, &kept_tally].into_iter().enumerate() {
            for (cause, tally) in &replay.tally {
                let row = totals.entry(cause.label().to_owned()).or_default();
                row[index * 2] += tally.samples;
                row[index * 2 + 1] += tally.episodes;
            }
        }
        for (time, state, summary) in &kept_tally.findings {
            if state == "SLOWDOWN" && summary.contains("below earlier sessions") {
                println!(
                    "  {} {time} {summary}",
                    kept[0].updated.split(' ').next().unwrap_or("")
                );
            }
        }
    }
    println!(
        "{generating} generating samples over {hours_generating:.1} h\n{:<26}{:>16}{:>16}",
        "REASON", "FRESH slow/ep", "KEPT slow/ep"
    );
    for (reason, [fresh_slow, fresh_episodes, kept_slow, kept_episodes]) in totals {
        println!(
            "{reason:<26}{:>16}{:>16}",
            format!("{fresh_slow}/{fresh_episodes}"),
            format!("{kept_slow}/{kept_episodes}")
        );
    }
}
