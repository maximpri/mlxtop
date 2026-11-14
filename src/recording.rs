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
    /// Lines of the file this version could not read as a sample.
    pub(crate) skipped: usize,
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
        // Take this sample's rows out: the Journal keeps at most 240, so
        // counting its growth stops working once it is full.
        journal.record_journal_events(previous.as_ref(), sample);
        for event in journal.signals.drain(..) {
            if matches!(
                event.state.as_str(),
                "SLOWDOWN" | "STALL" | "RESUMED" | "RECOVERED" | "SETTLED" | "ENDED"
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
    if replay.skipped > 0 {
        writeln!(
            out,
            "SKIPPED     {} line{} not readable by this version",
            replay.skipped,
            if replay.skipped == 1 { "" } else { "s" }
        )?;
    }
    writeln!(out, "FINDINGS    {started}")?;
    for (time, state, summary) in &replay.findings {
        writeln!(out, "{time}  {state:<9} {summary}")?;
    }
    Ok(())
}

/// `mlxtop replay FILE`.
pub(crate) fn run(path: &Path, thresholds: Thresholds, out: &mut dyn Write) -> io::Result<()> {
    let named =
        |error: io::Error| io::Error::new(error.kind(), format!("{}: {error}", path.display()));
    let (mut samples, skipped) =
        read_counted(BufReader::new(File::open(path).map_err(named)?)).map_err(named)?;
    if samples.is_empty() {
        return Err(io::Error::other(format!(
            "no samples in {}; record some with --record",
            path.display()
        )));
    }
    let mut result = replay(&mut samples, thresholds);
    result.skipped = skipped;
    write_report(out, &result)
}

#[cfg(test)]
#[path = "tests/recording.rs"]
mod tests;
