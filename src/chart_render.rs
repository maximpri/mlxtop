// SPDX-License-Identifier: MIT
//! Shared time-series scaling, stepped traces and captured-severity rendering.
use crate::config::Thresholds;
use crate::domain::{ChartMetric, ChartPoint, Sample, Tone};
use crate::formatting::{compact_tokens, rate};
use std::collections::VecDeque;
use std::time::Duration;
pub(crate) const CHART_VISUAL_DEADBAND_FRACTION: f64 = 0.25;

#[derive(Clone, Copy)]
pub(crate) struct TraceCell {
    pub(crate) glyph: char,
    pub(crate) tone: Tone,
    /// An area's body beneath its line, drawn as a quiet background.
    pub(crate) fill: bool,
}

/// How a chart draws its samples, by what the data is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChartForm {
    /// A level that persists between samples (RAM occupied, GPU busy): a line
    /// with the area beneath it filled, so it reads as how full.
    Area,
    /// A per-interval rate or ratio that comes and goes in bursts: one bar per
    /// sample standing on zero, with idle samples left empty.
    Bars,
}

pub(crate) fn chart_form(metric: ChartMetric) -> ChartForm {
    match metric {
        ChartMetric::Memory | ChartMetric::Gpu => ChartForm::Area,
        ChartMetric::Compression
        | ChartMetric::Swap
        | ChartMetric::Generation
        | ChartMetric::Prefill
        | ChartMetric::Disk
        | ChartMetric::Cache => ChartForm::Bars,
    }
}

#[derive(Clone, Copy)]
pub(crate) struct RenderPoint {
    pub(crate) value: Option<u64>,
    pub(crate) tone: Tone,
    pub(crate) break_before: bool,
    pub(crate) usual: Option<u64>,
}

pub(crate) fn normalize_chart_value(metric: ChartMetric, value: u64) -> u64 {
    match metric {
        ChartMetric::Generation
        | ChartMetric::Prefill
        | ChartMetric::Swap
        | ChartMetric::Disk
        | ChartMetric::Compression => value,
        ChartMetric::Cache | ChartMetric::Memory | ChartMetric::Gpu => value.min(100),
    }
}

pub(crate) fn chart_stats<'a, I>(points: I, metric: ChartMetric) -> (Option<u64>, Option<u64>)
where
    I: IntoIterator<Item = &'a ChartPoint>,
{
    let mut count = 0_u64;
    let mut total = 0_u64;
    let mut peak = 0_u64;
    for value in points.into_iter().filter_map(|point| point.value) {
        // Paging stats stay in bytes/s: a percent of the log scale would be
        // meaningless next to the byte-rate label shown for the live value.
        let value = if metric.is_byte_rate()
            || matches!(metric, ChartMetric::Generation | ChartMetric::Prefill)
        {
            value
        } else {
            normalize_chart_value(metric, value)
        };
        count += 1;
        total = total.saturating_add(value);
        peak = peak.max(value);
    }
    if count == 0 {
        (None, None)
    } else {
        (
            Some(total.checked_div(count).unwrap_or_default()),
            Some(peak),
        )
    }
}

pub(crate) fn chart_stats_for_width(
    history: &VecDeque<ChartPoint>,
    metric: ChartMetric,
    width: usize,
) -> (Option<u64>, Option<u64>) {
    let visible_start = history.len().saturating_sub(width);
    chart_stats(history.iter().skip(visible_start), metric)
}

pub(crate) fn chart_stat_label(metric: ChartMetric, value: Option<u64>) -> String {
    match metric {
        ChartMetric::Generation | ChartMetric::Prefill => value
            .map(|value| format!("{:.1}", value as f64 / 10.0))
            .unwrap_or_else(|| "—".into()),
        ChartMetric::Swap | ChartMetric::Compression | ChartMetric::Disk => {
            value.map(rate).unwrap_or_else(|| "—".into())
        }
        ChartMetric::Cache | ChartMetric::Memory | ChartMetric::Gpu => value
            .map(|value| format!("{value}%"))
            .unwrap_or_else(|| "—".into()),
    }
}

pub(crate) fn chart_scale(
    history: &VecDeque<ChartPoint>,
    metric: ChartMetric,
    width: usize,
) -> (u64, u64) {
    if matches!(metric, ChartMetric::Generation | ChartMetric::Prefill) {
        // Bars stand on zero, so their height is the rate itself.
        (
            0,
            crate::chart_scale::ceiling(
                history
                    .iter()
                    .skip(history.len().saturating_sub(width))
                    .filter_map(|point| point.value)
                    .max()
                    .unwrap_or(0),
                10,
            ),
        )
    } else if metric.is_byte_rate() {
        (
            0,
            crate::chart_scale::ceiling(
                history
                    .iter()
                    .skip(history.len().saturating_sub(width))
                    .filter_map(|point| point.value)
                    .max()
                    .unwrap_or(0),
                1,
            ),
        )
    } else {
        (0, 100)
    }
}

pub(crate) fn chart_axis_label(metric: ChartMetric, value: u64) -> String {
    if matches!(metric, ChartMetric::Generation | ChartMetric::Prefill) {
        if value >= 100_000 {
            compact_tokens(value / 10)
        } else if value.is_multiple_of(10) {
            (value / 10).to_string()
        } else {
            format!("{:.1}", value as f64 / 10.0)
        }
    } else if metric.is_byte_rate() {
        rate(value)
    } else {
        format!("{value}%")
    }
}

pub(crate) fn chart_display_value(metric: ChartMetric, value: u64, scale: (u64, u64)) -> u64 {
    if metric.is_byte_rate() || matches!(metric, ChartMetric::Generation | ChartMetric::Prefill) {
        (u128::from(value.saturating_sub(scale.0)) * 100
            / u128::from(scale.1.saturating_sub(scale.0).max(1)))
        .min(100) as u64
    } else {
        normalize_chart_value(metric, value)
    }
}

pub(crate) fn chart_window_label(samples: usize, interval: Duration) -> String {
    let seconds = (samples as u64).saturating_mul(interval.as_secs().max(1));
    if seconds >= 60 {
        format!("window {}m", seconds / 60)
    } else {
        format!("window {seconds}s")
    }
}

pub(crate) fn chart_columns(history: &VecDeque<ChartPoint>, width: usize) -> Vec<RenderPoint> {
    if width == 0 {
        return Vec::new();
    }

    let visible_start = history.len().saturating_sub(width);
    let left_padding = width.saturating_sub(history.len() - visible_start);
    let mut columns = vec![
        RenderPoint {
            value: None,
            tone: Tone::Muted,
            break_before: true,
            usual: None,
        };
        width
    ];

    for (offset, point) in history.iter().skip(visible_start).enumerate() {
        columns[left_padding + offset] = RenderPoint {
            value: point.value,
            tone: point.tone,
            break_before: point.value.is_none(),
            usual: point.usual,
        };
    }
    columns
}

// Spread an identical sample window over any plot width without averaging,
// skipping spikes, or connecting across absent observations.
pub(crate) fn stretch_chart_columns(points: &[RenderPoint], width: usize) -> Vec<RenderPoint> {
    if points.is_empty() {
        return Vec::new();
    }
    (0..width)
        .map(|column| points[column * points.len() / width])
        .collect()
}

pub(crate) fn chart_columns_for_plot(
    history: &VecDeque<ChartPoint>,
    width: usize,
    metric: ChartMetric,
    plot_height: usize,
    scale: (u64, u64),
) -> Vec<RenderPoint> {
    let mut columns = chart_columns(history, width);
    let visible_start = history.len().saturating_sub(width);
    let left_padding = width.saturating_sub(history.len() - visible_start);
    let plot_values = chart_plot_values_scaled(history, metric, plot_height, scale);
    for (offset, value) in plot_values.iter().skip(visible_start).enumerate() {
        columns[left_padding + offset].value = *value;
    }
    columns
}

pub(crate) fn chart_plot_values_scaled(
    history: &VecDeque<ChartPoint>,
    metric: ChartMetric,
    plot_height: usize,
    scale: (u64, u64),
) -> Vec<Option<u64>> {
    let deadband = chart_visual_deadband(plot_height);
    let mut anchor = None;
    let mut values = Vec::with_capacity(history.len());
    for point in history {
        let Some(value) = point.value else {
            anchor = None;
            values.push(None);
            continue;
        };
        let plotted = match anchor {
            Some(previous) if chart_display_delta(metric, previous, value, scale) <= deadband => {
                previous
            }
            _ => value,
        };
        anchor = Some(plotted);
        values.push(Some(plotted));
    }
    values
}

pub(crate) fn chart_visual_deadband(plot_height: usize) -> f64 {
    let drawable_rows = plot_height.saturating_sub(1);
    if drawable_rows == 0 {
        100.0
    } else {
        100.0 / drawable_rows as f64 * CHART_VISUAL_DEADBAND_FRACTION
    }
}

pub(crate) fn chart_display_delta(
    metric: ChartMetric,
    left: u64,
    right: u64,
    scale: (u64, u64),
) -> f64 {
    let left = chart_display_value(metric, left, scale) as f64;
    let right = chart_display_value(metric, right, scale) as f64;
    (left - right).abs()
}

/// A bar's cells from the bottom up for a 0–100 display value: full cells,
/// then the eighth-block that tops it. A measured rate above zero always shows.
pub(crate) fn bar_cells(value: u64, height: usize) -> Vec<char> {
    const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let eighths = (value.min(100) as usize * height * 8 + 50) / 100;
    let eighths = if value > 0 { eighths.max(1) } else { 0 };
    (0..height)
        .map(|row| BLOCKS[eighths.saturating_sub(row * 8).min(8)])
        .take_while(|glyph| *glyph != ' ')
        .collect()
}

/// A small horizontal meter for a 0–100 share, `cells` wide, filled in
/// eighths from the left; the unfilled rest is spaces for a track to show.
pub(crate) fn meter_cells(percent: f64, cells: usize) -> String {
    const PARTS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
    let eighths = (percent.clamp(0.0, 100.0) / 100.0 * (cells * 8) as f64).round() as usize;
    (0..cells)
        .map(|cell| match eighths.saturating_sub(cell * 8).min(8) {
            0 => ' ',
            part => PARTS[part - 1],
        })
        .collect()
}

pub(crate) fn trace_point(value: u64, height: usize) -> Option<(usize, char)> {
    if height == 0 {
        return None;
    }
    let value = value.min(100);
    let row_count = height.saturating_sub(1) as u64;
    let row_from_bottom = value.saturating_mul(row_count).saturating_add(50) / 100;
    let row = height - 1 - row_from_bottom.min(row_count) as usize;
    Some((row, '━'))
}

/**
 * How one chart segment should be coloured: which metric it belongs to, the
 * tones at each end of the segment, and the thresholds that band them.
 */
#[derive(Clone, Copy)]
pub(crate) struct TraceStyle {
    pub(crate) metric: ChartMetric,
    pub(crate) previous_tone: Tone,
    pub(crate) tone: Tone,
    pub(crate) thresholds: Thresholds,
    pub(crate) scale: (u64, u64),
}

pub(crate) fn trace_connector(
    cells: &mut [Vec<TraceCell>],
    column: usize,
    previous_row: usize,
    row: usize,
    style: TraceStyle,
) {
    let TraceStyle {
        metric,
        previous_tone,
        tone,
        thresholds,
        scale,
    } = style;
    if column >= cells.first().map(Vec::len).unwrap_or(0)
        || previous_row == row
        || previous_row >= cells.len()
        || row >= cells.len()
    {
        return;
    }
    let upper = previous_row.min(row);
    let lower = previous_row.max(row);
    let height = cells.len();
    for (offset, row_cells) in cells.iter_mut().enumerate().take(lower).skip(upper + 1) {
        let fraction = chart_row_value(offset, height);
        let display_value = if metric.is_byte_rate() {
            scale.0.saturating_add(
                (u128::from(scale.1.saturating_sub(scale.0)) * u128::from(fraction) / 100) as u64,
            )
        } else {
            fraction
        };
        row_cells[column] = TraceCell {
            glyph: '┃',
            tone: chart_transition_tone(metric, display_value, previous_tone, tone, thresholds),
            fill: false,
        };
    }
    cells[upper][column] = TraceCell {
        glyph: if row > previous_row { '┓' } else { '┏' },
        tone: if upper == previous_row {
            previous_tone
        } else {
            tone
        },
        fill: false,
    };
    cells[lower][column] = TraceCell {
        glyph: if row > previous_row { '┗' } else { '┛' },
        tone: if lower == previous_row {
            previous_tone
        } else {
            tone
        },
        fill: false,
    };
}

pub(crate) fn chart_row_value(row: usize, height: usize) -> u64 {
    let row_count = height.saturating_sub(1) as u64;
    if row_count == 0 {
        return 0;
    }
    (height.saturating_sub(1).saturating_sub(row) as u64)
        .saturating_mul(100)
        .checked_div(row_count)
        .unwrap_or(0)
}

pub(crate) fn chart_transition_tone(
    metric: ChartMetric,
    display_value: u64,
    previous_tone: Tone,
    tone: Tone,
    thresholds: Thresholds,
) -> Tone {
    match metric {
        ChartMetric::Generation | ChartMetric::Prefill => {
            if previous_tone == tone {
                tone
            } else {
                Tone::Muted
            }
        }
        // Pressure has no numeric relationship to the connector's RAM height.
        // Use the newly captured state; never synthesize a warning while
        // crossing an occupancy percentage between two normal observations.
        ChartMetric::Cache | ChartMetric::Memory | ChartMetric::Disk => tone,
        ChartMetric::Gpu => metric.tone(display_value, thresholds),
        ChartMetric::Compression => metric.tone(display_value, thresholds),
        // The chart plots all disk paging but grades what counted against the
        // system, so a connector never outranks both of its captured points.
        ChartMetric::Swap => {
            let rank = |tone: Tone| match tone {
                Tone::Red => 2,
                Tone::Yellow => 1,
                _ => 0,
            };
            let ceiling = if rank(previous_tone) >= rank(tone) {
                previous_tone
            } else {
                tone
            };
            let by_value = metric.tone(display_value, thresholds);
            if rank(by_value) > rank(ceiling) {
                ceiling
            } else {
                by_value
            }
        }
    }
}

pub(crate) fn chart_inactive_rate_label(metric: ChartMetric, sample: &Sample) -> String {
    match (metric, sample.llm_status.as_str()) {
        (ChartMetric::Generation, "prefilling") => "prefill active".into(),
        (ChartMetric::Generation, "generating") => "active".into(),
        (ChartMetric::Prefill, "prefilling") => "active".into(),
        (ChartMetric::Prefill, "generating") => "decode active".into(),
        (_, "idle" | "last result") => "idle".into(),
        (_, "waiting") => "waiting".into(),
        (_, "offline") => "offline".into(),
        _ => "—".into(),
    }
}
