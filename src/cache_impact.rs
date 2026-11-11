// SPDX-License-Identifier: MIT
//! What the runtime's prompt cache does for speed: how much of each prompt it
//! served instead of computing, the prefill time that saved, and what its SSD
//! tier is doing. Drawn as bars, not as raw rates.
use crate::analysis::DISK_ACTIVE_RATE;
use crate::domain::{ChartPoint, DecodeRead, Sample, Tone};
use crate::formatting::{compact_tokens, duration_label};
use crate::theme::{BLUE, EDGE, GREEN, MUTED, PANEL};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::collections::VecDeque;

/// The prefill speed cached tokens would otherwise have cost: the median of
/// the measured prefill samples, or the runtime's own figure without any.
pub(crate) fn prefill_speed(history: &VecDeque<ChartPoint>, sample: &Sample) -> Option<f64> {
    let mut measured: Vec<u64> = history
        .iter()
        .filter_map(|point| point.value)
        .filter(|value| *value > 0)
        .collect();
    measured.sort_unstable();
    measured
        .get(measured.len() / 2)
        .map(|value| *value as f64 / 10.0)
        .or(sample.llm_prefill_tps)
        .filter(|speed| speed.is_finite() && *speed > 0.0)
}

/// Seconds of prefill the cache saved: its tokens at the measured prefill speed.
pub(crate) fn saved_seconds(cached: u64, speed: Option<f64>) -> Option<f64> {
    speed.map(|speed| cached as f64 / speed)
}

/// The session's prompt tokens and the share of them the cache served.
pub(crate) fn session(sample: &Sample) -> Option<(u64, u64)> {
    sample
        .llm_total_prompt_tokens
        .zip(sample.llm_total_cached_tokens)
        .filter(|(prompt, cached)| *prompt > 0 && cached <= prompt)
}

/// The newest prompt whose cached share the runtime reported.
pub(crate) fn last_prompt(sample: &Sample) -> Option<(u64, u64)> {
    sample
        .llm_requests
        .iter()
        .rev()
        .filter(|request| request.prompt > 0)
        .find_map(|request| {
            request
                .cached
                .filter(|cached| *cached <= request.prompt)
                .map(|cached| (request.prompt, cached))
        })
}

/// The title's reading: the prefill time the cache saved this session.
pub(crate) fn headline(sample: &Sample, speed: Option<f64>, width: u16) -> String {
    let Some((_, cached)) = session(sample) else {
        if sample.disk.weights > 0 {
            return "streaming weights".into();
        }
        return sample
            .llm_cache_efficiency
            .map(|reuse| format!("reuse {reuse:.0}%"))
            .unwrap_or_else(|| "—".into());
    };
    match saved_seconds(cached, speed) {
        Some(seconds) if width >= 40 => format!("saved ≈{} of prefill", duration_label(seconds)),
        Some(seconds) => format!("saved ≈{}", duration_label(seconds)),
        None => format!("{} cached", compact_tokens(cached)),
    }
}

/// What the SSD tier did in the last interval, from the runtime's block
/// counts, or the disk traffic where the runtime keeps no such counts.
pub(crate) fn activity(sample: &Sample) -> Option<(&'static str, Tone)> {
    let disk = sample.disk;
    let traffic = disk.read.zip(disk.write).map(|(r, w)| r.saturating_add(w));
    Some(if disk.weights > 0 {
        ("streaming weights from disk", Tone::Blue)
    } else if disk.evictions > 0 {
        ("evicting old prefixes", Tone::Yellow)
    } else if disk.loads > 0 && disk.saves > 0 {
        ("loading and saving prefixes", Tone::Cyan)
    } else if disk.loads > 0 {
        ("loading prefixes from SSD", Tone::Cyan)
    } else if disk.saves > 0 {
        ("saving prefixes to SSD", Tone::Muted)
    } else {
        match traffic? {
            0 => ("disk idle", Tone::Muted),
            rate if rate >= DISK_ACTIVE_RATE => ("disk busy", Tone::Yellow),
            _ => ("light disk traffic", Tone::Muted),
        }
    })
}

/// One reuse bar: the cached share green, the computed rest dim, and what it
/// amounts to on the right.
pub(crate) fn draw_reuse(
    frame: &mut Frame,
    row: Rect,
    label: &str,
    (prompt, cached): (u64, u64),
    speed: Option<f64>,
) {
    if row.is_empty() {
        return;
    }
    let share = cached as f64 / prompt as f64;
    let percent = format!("{:.0}%", share * 100.0);
    let saved = saved_seconds(cached, speed).map(|seconds| format!("≈{}", duration_label(seconds)));
    let width = usize::from(row.width);
    let prefix = format!("{label:<8}");
    // The longest note that still leaves a bar of eight cells.
    let note = [
        saved
            .as_ref()
            .map(|saved| format!("{percent} of {} · {saved} saved", compact_tokens(prompt))),
        saved
            .as_ref()
            .map(|saved| format!("{percent} · {saved} saved")),
        Some(format!("{percent} of {}", compact_tokens(prompt))),
        Some(percent.clone()),
    ]
    .into_iter()
    .flatten()
    .find(|note| prefix.len() + Line::from(note.as_str()).width() + 10 <= width)
    .unwrap_or_default();
    let bar = width
        .saturating_sub(prefix.len() + Line::from(note.as_str()).width() + 1)
        .max(1);
    let filled = ((share * bar as f64).round() as usize).min(bar);
    let line = Line::from(vec![
        Span::styled(prefix, Style::default().fg(MUTED)),
        Span::styled("█".repeat(filled), Style::default().fg(GREEN)),
        Span::styled("█".repeat(bar - filled), Style::default().fg(EDGE)),
        Span::styled(format!(" {note}"), Style::default().fg(GREEN)),
    ]);
    frame.render_widget(Paragraph::new(line).style(Style::default().bg(PANEL)), row);
}

/// How decoding depended on the disk: the share of decoding samples that read
/// weights from it, and the median speed with and without those reads.
pub(crate) fn weights(reads: &VecDeque<DecodeRead>) -> Option<(f64, Option<f64>, Option<f64>)> {
    let streamed = reads.iter().filter(|read| read.from_disk).count();
    if streamed == 0 {
        return None;
    }
    let median = |from_disk: bool| {
        let mut speeds: Vec<f64> = reads
            .iter()
            .filter(|read| read.from_disk == from_disk)
            .map(|read| read.speed)
            .collect();
        speeds.sort_by(f64::total_cmp);
        speeds.get(speeds.len() / 2).copied()
    };
    Some((
        streamed as f64 / reads.len() as f64,
        median(true),
        median(false),
    ))
}

/// The weights bar: decoding samples that read weights from disk, blue, and
/// what that cost in speed against decoding from memory.
pub(crate) fn draw_weights(
    frame: &mut Frame,
    row: Rect,
    (share, from_disk, from_memory): (f64, Option<f64>, Option<f64>),
) {
    if row.is_empty() {
        return;
    }
    let percent = format!("{:.0}%", share * 100.0);
    let speeds = match (from_disk, from_memory) {
        (Some(disk), Some(memory)) => Some(format!("{disk:.0} vs {memory:.0} tok/s")),
        (Some(disk), None) => Some(format!("{disk:.0} tok/s")),
        _ => None,
    };
    let width = usize::from(row.width);
    let prefix = format!("{:<8}", "weights");
    let note = [
        speeds
            .as_ref()
            .map(|speeds| format!("{percent} of decode from disk · {speeds}")),
        speeds
            .as_ref()
            .map(|speeds| format!("{percent} · {speeds}")),
        Some(format!("{percent} of decode from disk")),
        Some(percent.clone()),
    ]
    .into_iter()
    .flatten()
    .find(|note| prefix.len() + Line::from(note.as_str()).width() + 10 <= width)
    .unwrap_or_default();
    let bar = width
        .saturating_sub(prefix.len() + Line::from(note.as_str()).width() + 1)
        .max(1);
    let filled = ((share * bar as f64).round() as usize).min(bar);
    let line = Line::from(vec![
        Span::styled(prefix, Style::default().fg(MUTED)),
        Span::styled("█".repeat(filled), Style::default().fg(BLUE)),
        Span::styled("█".repeat(bar - filled), Style::default().fg(EDGE)),
        Span::styled(format!(" {note}"), Style::default().fg(BLUE)),
    ]);
    frame.render_widget(Paragraph::new(line).style(Style::default().bg(PANEL)), row);
}
