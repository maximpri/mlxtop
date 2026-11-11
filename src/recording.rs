// SPDX-License-Identifier: MIT
//! Recordings: every sample written as one JSON line while mlxtop runs, and
//! read back through the same detection engine and Journal. A rule can then
//! be checked against a real incident instead of a reconstruction, and a
//! recording can go with a bug report.
use crate::analysis::CorrelationEngine;
use crate::collector::Collector;
use crate::config::{Config, Thresholds};
use crate::domain::{Phase, Sample};
use crate::host::{Host, Platform};
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static RECORDING: OnceLock<Mutex<File>> = OnceLock::new();

/// Start appending every sample to `path`.
pub(crate) fn start(path: &Path) -> io::Result<()> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    RECORDING
        .set(Mutex::new(file))
        .map_err(|_| io::Error::other("already recording"))
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

/// Every sample in a recording, skipping lines that are not one.
pub(crate) fn read(reader: impl BufRead) -> Vec<Sample> {
    reader
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str(&line).ok())
        .collect()
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

/// What a recording's samples amount to under today's rules.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Replay {
    pub(crate) samples: usize,
    pub(crate) generating: usize,
    pub(crate) prefilling: usize,
    /// Samples inside a slowdown or stall, by phase.
    pub(crate) slow_generation: usize,
    pub(crate) slow_prefill: usize,
    /// Journal rows about the model's speed, with their time.
    pub(crate) findings: Vec<(String, String, String)>,
}

/// Run `samples` through the detection engine and Journal.
pub(crate) fn replay(samples: &mut [Sample], thresholds: Thresholds) -> Replay {
    let mut engine = CorrelationEngine::default();
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
        }
        let before = journal.signals.len();
        journal.record_journal_events(previous.as_ref(), sample);
        let added = journal.signals.len() - before.min(journal.signals.len());
        for event in journal.signals.iter().rev().take(added).rev() {
            if matches!(
                event.state.as_str(),
                "SLOWDOWN" | "STALL" | "RESUMED" | "RECOVERED" | "SETTLED"
            ) {
                replay.findings.push((
                    event.time.clone(),
                    event.state.clone(),
                    event.summary.clone(),
                ));
            }
        }
        previous = Some(sample.clone());
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
    writeln!(
        out,
        "GENERATION  {} samples · {} in a slowdown or stall ({})",
        replay.generating,
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
    let started = replay
        .findings
        .iter()
        .filter(|(_, state, _)| matches!(state.as_str(), "SLOWDOWN" | "STALL"))
        .count();
    writeln!(out, "FINDINGS    {started}")?;
    for (time, state, summary) in &replay.findings {
        writeln!(out, "{time}  {state:<9} {summary}")?;
    }
    Ok(())
}

/// `mlxtop replay FILE`.
pub(crate) fn run(path: &Path, thresholds: Thresholds, out: &mut dyn Write) -> io::Result<()> {
    let mut samples = read(BufReader::new(File::open(path)?));
    if samples.is_empty() {
        return Err(io::Error::other(format!(
            "no samples in {}; record some with --record",
            path.display()
        )));
    }
    write_report(out, &replay(&mut samples, thresholds))
}

#[cfg(test)]
#[path = "tests/recording.rs"]
mod tests;
