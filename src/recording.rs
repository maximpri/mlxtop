// SPDX-License-Identifier: MIT
//! Recordings: every sample written as one JSON line while mlxtop runs, and
//! read back through the same detection engine and Journal. A rule can then
//! be checked against a real incident instead of a reconstruction, and a
//! recording can go with a bug report. A replay counts what fired: the
//! slowdowns and stalls by the reason given, and what requests lost before
//! their first tokens by finding.
use crate::analysis::CorrelationEngine;
use crate::collector::Collector;
use crate::config::{Config, Thresholds};
use crate::domain::{CorrelationCause, Phase, Sample};
use crate::formatting::duration_label;
use crate::host::{Host, Platform};
use crate::request_findings::STATES;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static RECORDING: OnceLock<Mutex<File>> = OnceLock::new();

/// Start appending every sample to `path`.
pub(crate) fn start(path: &Path) -> io::Result<()> {
    let file = open_private(path)?;
    RECORDING
        .set(Mutex::new(file))
        .map_err(|_| io::Error::other("already recording"))
}

/// Open `path` for appending, created readable by its owner only: a
/// recording is the user's to share, not the machine's.
pub(crate) fn open_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

/// Append one sample, when recording. A failed write never stops sampling.
pub(crate) fn write(sample: &Sample) {
    let Some(file) = RECORDING.get() else {
        return;
    };
    if let (Ok(mut file), Ok(line)) = (file.lock(), serde_json::to_string(sample)) {
        let _ = writeln!(file, "{line}");
    }
}

/// Every sample in a recording, and how many non-empty lines were not one.
/// A line must be a JSON object with a sample's `updated` field: any other
/// object would otherwise read as an all-default sample.
pub(crate) fn read_counted(reader: impl BufRead) -> io::Result<(Vec<Sample>, usize)> {
    let mut samples = Vec::new();
    let mut skipped = 0;
    for line in reader.split(b'\n') {
        let line = line?;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let sample = serde_json::from_slice::<serde_json::Value>(&line)
            .ok()
            .filter(|value| value.get("updated").is_some())
            .and_then(|value| serde_json::from_value::<Sample>(value).ok());
        match sample {
            Some(sample) => samples.push(sample),
            None => skipped += 1,
        }
    }
    Ok((samples, skipped))
}

/// A host with nothing on it: replay reads samples, not the machine.
struct Nothing;

impl Host for Nothing {
    fn command(&self, _: &str, _: &[&str]) -> Option<String> {
        None
    }
    fn read_file(&self, _: &Path) -> Option<String> {
        None
    }
    fn read_dir(&self, _: &Path) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// How often one reason was given for a slowdown or stall.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Tally {
    /// Samples inside a slowdown or stall with this reason.
    pub(crate) samples: usize,
    /// Slowdowns and stalls the Journal began with this reason: the rows a
    /// person watching would have read.
    pub(crate) episodes: usize,
}

/// How often one finding about what requests lost before their first
/// tokens was told (a long prompt read in full after a cached one, a model
/// load, a full KV cache, two at once that bought nothing), and its cost.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Told {
    /// Its Journal state, and its name in the tally.
    pub(crate) state: &'static str,
    pub(crate) name: &'static str,
    /// The rows the Journal wrote of it.
    pub(crate) rows: usize,
    /// Seconds first tokens lost to it, as the session summary counts them;
    /// none for a full KV cache, whose cost is not counted.
    pub(crate) lost: Option<f64>,
}

/// The Journal state and tally name of a finding about what requests lost
/// before their first tokens, if `state` is one.
fn before_first_token(state: &str) -> Option<(&'static str, &'static str)> {
    STATES.into_iter().find(|(known, _)| *known == state)
}

/// What a recording's samples amount to under today's rules.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Replay {
    pub(crate) samples: usize,
    pub(crate) generating: usize,
    pub(crate) prefilling: usize,
    /// Samples inside a slowdown or stall, by phase.
    pub(crate) slow_generation: usize,
    pub(crate) slow_prefill: usize,
    /// Seconds an answer was being written or had stalled, counted as the
    /// session summary counts them: what episodes are counted per hour of.
    pub(crate) generating_seconds: f64,
    /// Each reason given for a slowdown or stall, in the order first given.
    pub(crate) tally: Vec<(CorrelationCause, Tally)>,
    /// Each finding about what requests lost before their first tokens, in
    /// the order first told.
    pub(crate) before_first_token: Vec<Told>,
    /// Journal rows about the model's speed and about what requests lost
    /// before their first tokens, with their time.
    pub(crate) findings: Vec<(String, String, String)>,
    /// Lines of the file this version could not read as a sample.
    pub(crate) skipped: usize,
}

impl Replay {
    /// The tally of `cause`, empty until it is first given.
    fn reason(&mut self, cause: CorrelationCause) -> &mut Tally {
        let index = match self.tally.iter().position(|(given, _)| *given == cause) {
            Some(index) => index,
            None => {
                self.tally.push((cause, Tally::default()));
                self.tally.len() - 1
            }
        };
        &mut self.tally[index].1
    }

    /// The count of the finding told as `state`, empty until first told.
    fn told(&mut self, (state, name): (&'static str, &'static str)) -> &mut Told {
        let index = match self
            .before_first_token
            .iter()
            .position(|told| told.state == state)
        {
            Some(index) => index,
            None => {
                self.before_first_token.push(Told {
                    state,
                    name,
                    rows: 0,
                    lost: None,
                });
                self.before_first_token.len() - 1
            }
        };
        &mut self.before_first_token[index]
    }
}

/// Run `samples` through a fresh detection engine and the Journal.
pub(crate) fn replay(samples: &mut [Sample], thresholds: Thresholds) -> Replay {
    replay_with(samples, thresholds, &mut CorrelationEngine::default())
}

/// Run `samples` through `engine` and the Journal: a fresh engine, or one
/// that judges against usual speeds kept across sessions.
pub(crate) fn replay_with(
    samples: &mut [Sample],
    thresholds: Thresholds,
    engine: &mut CorrelationEngine,
) -> Replay {
    let config = Config::default();
    let mut journal = Collector::with_host(
        samples.len().max(20),
        config,
        Box::new(Nothing),
        Platform::current(),
        None,
    );
    journal.thresholds = thresholds;
    let mut replay = Replay {
        samples: samples.len(),
        ..Replay::default()
    };
    let mut previous: Option<Sample> = None;
    for sample in samples.iter_mut() {
        sample.correlation = engine.observe(sample, thresholds);
        let insight = &sample.correlation;
        if sample.llm_generation_tps_live && sample.llm_generation_tps.is_some_and(|tps| tps > 0.0)
        {
            replay.generating += 1;
        } else if sample.llm_prefill_tps_live {
            replay.prefilling += 1;
        }
        if insight.is_material_drop() {
            match insight.phase {
                Phase::Generation => replay.slow_generation += 1,
                Phase::Prefill => replay.slow_prefill += 1,
            }
            replay.reason(insight.cause).samples += 1;
        }
        // Take this sample's rows out: the Journal keeps at most 240, so
        // counting its growth stops working once it is full.
        journal.record_journal_events(previous.as_ref(), sample);
        for event in journal.signals.drain(..) {
            // A slowdown or stall begins with its row, which gives the
            // reason this sample was judged slow for.
            if matches!(event.state.as_str(), "SLOWDOWN" | "STALL") {
                replay.reason(sample.correlation.cause).episodes += 1;
            }
            // What requests lost before their first tokens: a row each.
            let told = before_first_token(&event.state);
            if let Some(finding) = told {
                replay.told(finding).rows += 1;
            }
            if told.is_some()
                || matches!(
                    event.state.as_str(),
                    "SLOWDOWN" | "STALL" | "RESUMED" | "RECOVERED" | "SETTLED" | "ENDED"
                )
            {
                replay.findings.push((
                    event.time.clone(),
                    event.state.clone(),
                    event.summary.clone(),
                ));
            }
        }
        journal.session.observe(sample);
        previous = Some(sample.clone());
    }
    replay.generating_seconds = journal.session.generating;
    for told in &mut replay.before_first_token {
        told.lost = journal.session.request_findings.lost_to(told.state);
    }
    replay
}

/// The replay as text: what fired, and how often.
pub(crate) fn write_report(out: &mut dyn Write, replay: &Replay) -> io::Result<()> {
    let share = |part: usize, whole: usize| {
        if whole == 0 {
            "—".to_owned()
        } else {
            format!("{:.1}%", part as f64 * 100.0 / whole as f64)
        }
    };
    writeln!(out, "SAMPLES     {}", replay.samples)?;
    let hours = replay.generating_seconds / 3600.0;
    writeln!(
        out,
        "GENERATION  {} samples{} · {} in a slowdown or stall ({})",
        replay.generating,
        if hours > 0.0 {
            format!(" over {}", duration_label(replay.generating_seconds))
        } else {
            String::new()
        },
        replay.slow_generation,
        share(replay.slow_generation, replay.generating)
    )?;
    writeln!(
        out,
        "PREFILL     {} samples · {} in a slowdown ({})",
        replay.prefilling,
        replay.slow_prefill,
        share(replay.slow_prefill, replay.prefilling)
    )?;
    let per_hour = |count: usize| {
        if hours > 0.0 {
            format!("{:.2}", count as f64 / hours)
        } else {
            "—".to_owned()
        }
    };
    // How often each reason fires: the count a new rule is checked by. The
    // most frequent first.
    if !replay.tally.is_empty() {
        let mut reasons: Vec<&(CorrelationCause, Tally)> = replay.tally.iter().collect();
        reasons.sort_by(|(cause, tally), (other, than)| {
            than.episodes
                .cmp(&tally.episodes)
                .then(than.samples.cmp(&tally.samples))
                .then((*cause as u8).cmp(&(*other as u8)))
        });
        writeln!(
            out,
            "{:<26}{:>6}{:>10}{:>10}",
            "REASON", "SLOW", "EPISODES", "PER HOUR"
        )?;
        for (cause, tally) in reasons {
            writeln!(
                out,
                "{:<26}{:>6}{:>10}{:>10}",
                cause.label(),
                tally.samples,
                tally.episodes,
                per_hour(tally.episodes)
            )?;
        }
    }
    // What requests lost before their first tokens, counted the same way:
    // the rows the Journal wrote of each finding, those rows per hour of
    // generation, and the time first tokens lost to it. The most told
    // first; a stable sort keeps equal counts in the order first told.
    if !replay.before_first_token.is_empty() {
        let mut told: Vec<&Told> = replay.before_first_token.iter().collect();
        told.sort_by_key(|told| std::cmp::Reverse(told.rows));
        writeln!(
            out,
            "{:<26}{:>6}{:>10}{:>10}",
            "BEFORE FIRST TOKEN", "ROWS", "PER HOUR", "LOST"
        )?;
        for told in told {
            writeln!(
                out,
                "{:<26}{:>6}{:>10}{:>10}",
                told.name,
                told.rows,
                per_hour(told.rows),
                told.lost.map_or_else(|| "—".to_owned(), duration_label)
            )?;
        }
    }
    // The slowdowns and stalls begun, and the findings before first tokens.
    let started = replay
        .findings
        .iter()
        .filter(|(_, state, _)| {
            matches!(state.as_str(), "SLOWDOWN" | "STALL") || before_first_token(state).is_some()
        })
        .count();
    if replay.skipped > 0 {
        writeln!(
            out,
            "SKIPPED     {} line{} not readable by this version",
            replay.skipped,
            if replay.skipped == 1 { "" } else { "s" }
        )?;
    }
    writeln!(out, "FINDINGS    {started}")?;
    // The state column fits the longest state told (`KV CACHE FULL`), and
    // `RECOVERED` at least.
    let width = replay
        .findings
        .iter()
        .map(|(_, state, _)| state.chars().count())
        .fold(9, usize::max);
    for (time, state, summary) in &replay.findings {
        writeln!(out, "{time}  {state:<width$} {summary}")?;
    }
    Ok(())
}

/// `mlxtop replay FILE [--baselines KEPT]`: with a baseline file, work the
/// recording has not yet done is judged against the usual speeds kept in
/// it, as a live session would be; the file is read, never written.
pub(crate) fn run(
    path: &Path,
    thresholds: Thresholds,
    baselines: Option<&Path>,
    out: &mut dyn Write,
) -> io::Result<()> {
    let named =
        |error: io::Error| io::Error::new(error.kind(), format!("{}: {error}", path.display()));
    let engine = baselines
        .map(|kept| {
            crate::baseline_store::Baselines::read_only(kept).map(CorrelationEngine::with_baselines)
        })
        .transpose()?;
    let (mut samples, skipped) =
        read_counted(BufReader::new(File::open(path).map_err(named)?)).map_err(named)?;
    if samples.is_empty() {
        return Err(io::Error::other(format!(
            "no samples in {}; record some with --record",
            path.display()
        )));
    }
    let mut result = match engine {
        Some(mut engine) => replay_with(&mut samples, thresholds, &mut engine),
        None => replay(&mut samples, thresholds),
    };
    result.skipped = skipped;
    write_report(out, &result)
}

#[cfg(test)]
#[path = "tests/recording.rs"]
mod tests;
