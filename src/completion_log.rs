// SPDX-License-Identifier: MIT
//! Fallback completion counters from local oMLX logs; never prompt or response content.
use crate::domain::LlmLogStats;
use std::fs::File;
use std::io::SeekFrom;
use std::io::{Read, Seek};
use std::path::Path;
pub(crate) fn read_llm_stats(home: Option<&Path>) -> LlmLogStats {
    let Some(home) = home else {
        return LlmLogStats::default();
    };
    let candidates = [
        home.join(".omlx-coding/logs/server.log"),
        home.join(".omlx-coding/logs/launchd.stdout.log"),
        home.join(".omlx-coding/logs/launchd.stderr.log"),
    ];
    candidates
        .into_iter()
        .filter_map(|path| read_latest_completion(&path))
        .max_by(|left, right| match (left.observed_at, right.observed_at) {
            (Some(left), Some(right)) => left.cmp(&right),
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (None, None) => std::cmp::Ordering::Equal,
        })
        .unwrap_or_default()
}

pub(crate) fn read_latest_completion(path: &Path) -> Option<LlmLogStats> {
    let mut file = File::open(path).ok()?;
    let observed_at = file
        .metadata()
        .ok()
        .and_then(|metadata| metadata.modified().ok());
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(256 * 1024);
    file.seek(SeekFrom::Start(start)).ok()?;
    // The tail can start inside a multi-byte character; decode what is there.
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut stats = text
        .lines()
        .filter_map(parse_llm_completion_line)
        .next_back()?;
    stats.observed_at = observed_at;
    Some(stats)
}

pub(crate) fn parse_llm_completion_line(line: &str) -> Option<LlmLogStats> {
    let (marker, marker_start) = if let Some(start) = line.find("Responses API: model=") {
        ("Responses API: model=", start)
    } else {
        let start = line.find("Chat completion: model=")?;
        ("Chat completion: model=", start)
    };
    let start = marker_start + marker.len();
    let (model, rest) = line[start..].split_once(", ")?;
    let (output, rest) = rest.split_once(" tokens in ")?;
    let output_tokens = output.trim().parse().ok()?;
    let (seconds, rest) = rest.split_once("s (")?;
    seconds.trim().parse::<f64>().ok()?;
    let (throughput, rest) = rest.split_once(" tok/s)")?;
    let tokens_per_second = throughput.trim().parse().ok()?;
    let prompt_tokens = rest.find("prompt: ").and_then(|prompt_start| {
        rest[prompt_start + "prompt: ".len()..]
            .split(',')
            .next()?
            .trim()
            .parse()
            .ok()
    });
    Some(LlmLogStats {
        model: Some(model.trim().into()),
        tokens_per_second: Some(tokens_per_second),
        output_tokens: Some(output_tokens),
        prompt_tokens,
        observed_at: None,
    })
}
