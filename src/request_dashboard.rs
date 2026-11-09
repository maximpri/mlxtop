// SPDX-License-Identifier: MIT
use crate::config::Thresholds;
use crate::domain::{ChartMetric, Sample, TelemetrySource};

/// Prompts smaller than this keep a neutral CACHED color.
const CACHE_GRADED_PROMPT: u64 = 4096;
use crate::chart_render::meter_cells;
use crate::formatting::{
    compact_label, compact_tokens, progress_percent, telemetry_age, time_left_label,
};
use crate::request_history::{same_request, Entry, History};
use crate::theme::{BLUE, CYAN, DIM, EDGE, GREEN, MUTED, PANEL};
use crate::{chart_scale, domain};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::time::{Duration, SystemTime};
fn exact(n: u64) -> String {
    let digits = n.to_string();
    let mut output = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            output.push(',');
        }
        output.push(c);
    }
    output
}

/// Local time when the offset is known, matching Journal; otherwise labeled UTC.
fn clock_stamp(at: SystemTime, utc_offset: Option<i32>) -> String {
    let Ok(elapsed) = at.duration_since(SystemTime::UNIX_EPOCH) else {
        return "--:--:--".into();
    };
    let shifted =
        i64::try_from(elapsed.as_secs()).unwrap_or(0) + i64::from(utc_offset.unwrap_or(0));
    let suffix = if utc_offset.is_some() { "" } else { " UTC" };
    let seconds = shifted.rem_euclid(86_400);
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}{suffix}")
}

/// The change from the request before, when there is one to compare with.
fn compact_comparison(current: &domain::RequestUsage, previous: Option<&Entry>) -> String {
    let Some(previous) = previous else {
        return String::new();
    };
    if current.provider != previous.usage.provider || current.model != previous.usage.model {
        return String::new();
    }
    let sign = if current.prompt >= previous.usage.prompt {
        "+"
    } else {
        "−"
    };
    let amount = current.prompt.abs_diff(previous.usage.prompt);
    let percent = if previous.usage.prompt > 0 {
        format!(
            " ({sign}{:.1}%)",
            amount as f64 / previous.usage.prompt as f64 * 100.0
        )
    } else {
        String::new()
    };
    format!("Δ {sign}{}{percent} vs previous", exact(amount))
}

fn history_window(history: &History, index: usize, width: u16, zoom: u16) -> (usize, usize, u16) {
    let width = width.max(1);
    let label_width = history
        .entries
        .iter()
        .take(index + 1)
        .rev()
        .take(usize::from(width / 6).max(1))
        .map(|entry| compact_tokens(entry.usage.prompt).len() as u16)
        .max()
        .unwrap_or(1)
        .max(4);
    // Newest requests stay at the right edge, just like the time series.
    // A short history must not stretch its slots across the plot: one request
    // would float in the middle and move when another request arrived.
    let slot_width = (label_width + 2).saturating_mul(zoom.max(1)).min(width);
    let visible = (index + 1).min(usize::from(width / slot_width));
    (index + 1 - visible, visible, slot_width)
}

fn comparable(a: &domain::RequestUsage, b: &domain::RequestUsage) -> bool {
    a.provider == b.provider && a.model == b.model
}

fn size_summary(history: &History, index: usize, width: u16) -> Line<'static> {
    let current = &history.entries[index].usage;
    let mut sizes: Vec<_> = history
        .entries
        .iter()
        .take(index + 1)
        .rev()
        .take_while(|entry| comparable(current, &entry.usage))
        .take(8)
        .map(|entry| entry.usage.prompt)
        .collect();
    sizes.sort_unstable();
    // One request has no typical size; the bar labels carry each size.
    if sizes.len() == 1 {
        return Line::default();
    }
    let middle = sizes.len() / 2;
    let sum = u128::from(sizes[middle]) + u128::from(sizes[(sizes.len() - 1) / 2]);
    // Token counts are whole numbers; round an even-sized median half up.
    let median = exact(sum.div_ceil(2) as u64);
    let mut line = Line::from(Span::styled(
        format!(" MEDIAN {median} tokens"),
        Style::default().fg(MUTED),
    ));
    append_detail(&mut line, format!("RECENT {}", sizes.len()), MUTED, width);
    if line.width() < usize::from(width) {
        line.spans.push(Span::raw(" "));
    }
    line
}

/// Plot rows the bars keep before a row of generated tokens goes under them.
const OUTPUT_ROW_MIN_PLOT: u16 = 4;

/// The tokens a bar's request has generated, so the size above needs no subtraction.
fn output_label(usage: &domain::RequestUsage, width: u16) -> String {
    let Some(output) = usage.output.filter(|output| *output > 0) else {
        return String::new();
    };
    let text = compact_tokens(output);
    if text.chars().count() <= usize::from(width) {
        text
    } else {
        String::new()
    }
}

fn is_live(entry: &Entry, sample: &Sample, now: SystemTime) -> bool {
    sample.llm_source == TelemetrySource::Live
        && sample.llm_status != "stale"
        && !entry.usage.completed
        && now
            .duration_since(entry.last_seen)
            .is_ok_and(|age| age <= Duration::from_secs(5))
        && sample.llm_observed_at.is_some_and(|at| {
            now.duration_since(at)
                .is_ok_and(|age| age <= Duration::from_secs(5))
        })
        && sample
            .llm_requests
            .iter()
            .any(|request| same_request(&entry.usage, request))
}

fn append_detail(line: &mut Line<'static>, text: String, color: Color, width: u16) {
    if text.is_empty() {
        return;
    }
    // The first detail on a line needs no separator before it.
    let separator = if line.width() == 0 { "" } else { " · " };
    let span = Span::styled(format!("{separator}{text}"), Style::default().fg(color));
    if line.width() + span.width() <= usize::from(width) {
        line.spans.push(span);
    }
}

/// The generation chart already shows the live aggregate rate. Repeat a live
/// request's speed only when it differs, as with concurrent requests.
fn repeats_generation_rate(entry: &Entry, sample: &Sample) -> bool {
    sample.llm_generation_tps_live
        && entry
            .output_speed
            .zip(sample.llm_generation_tps)
            .is_some_and(|(speed, rate)| (speed.tps - rate).abs() < 0.05)
}

/// A live request still prefilling: how far its prompt has got as a bar,
/// with the time left at the current prefill rate. Other requests show their
/// output instead.
fn progress_or_output(entry: &Entry, live: bool, sample: &Sample, width: u16) -> Line<'static> {
    let usage = &entry.usage;
    let Some(done) = usage
        .prefilled
        .filter(|done| live && !usage.queued && *done < usage.prompt)
    else {
        return output_summary(entry, live, sample, width);
    };
    let prompt = usage.prompt.max(1);
    let percent = progress_percent(done, prompt);
    let left = sample
        .llm_prefill_tps_live
        .then_some(sample.llm_prefill_tps)
        .flatten()
        .filter(|rate| *rate > 0.0)
        .map(|rate| time_left_label((prompt - done) as f64 / rate));
    let left = left
        .map(|left| format!(" · {left} left"))
        .unwrap_or_default();
    let label = "PREFILL ";
    // Whole details fit or drop: the percentage and time left outrank counts.
    let width = usize::from(width);
    let counts = format!(" · {} / {}", exact(done), exact(prompt));
    let tail = [
        format!(" {percent}%{counts}{left}"),
        format!(" {percent}%{left}"),
        format!(" {percent}%"),
    ]
    .into_iter()
    .find(|tail| label.len() + 6 + Line::from(tail.as_str()).width() <= width)
    .unwrap_or_else(|| format!(" {percent}%"));
    let bar = width
        .saturating_sub(label.len() + Line::from(tail.as_str()).width())
        .min(30);
    let mut line = Line::from(Span::styled(label, Style::default().fg(CYAN)));
    if bar >= 6 {
        line.spans.push(Span::styled(
            meter_cells(percent as f64, bar),
            Style::default().fg(CYAN).bg(EDGE),
        ));
    }
    line.spans
        .push(Span::styled(tail, Style::default().fg(CYAN)));
    line
}

fn output_summary(entry: &Entry, live: bool, sample: &Sample, width: u16) -> Line<'static> {
    let live_duplicate = live && repeats_generation_rate(entry, sample);
    let speed = entry.output_speed.map(|speed| {
        let state = if speed.completed {
            "AVG"
        } else if live && entry.usage.output_tps == Some(speed.tps) {
            "LIVE"
        } else {
            "LAST"
        };
        (format!("{state} {:.1} tok/s", speed.tps), state)
    });
    let speed_label = speed
        .as_ref()
        .map(|(label, _)| label.as_str())
        .unwrap_or("SPEED —");
    let mut count = entry.usage.output.map(exact).unwrap_or_else(|| "—".into());
    if format!("OUT {count} · {speed_label}").chars().count() > usize::from(width) {
        count = entry
            .usage
            .output
            .map(compact_tokens)
            .unwrap_or_else(|| "—".into());
    }
    let mut line = Line::from(Span::styled(
        format!("OUT {count}"),
        Style::default().fg(CYAN),
    ));
    if !(live_duplicate && speed.as_ref().is_some_and(|(_, state)| *state == "LIVE")) {
        append_detail(
            &mut line,
            speed_label.into(),
            if speed.is_some() { CYAN } else { MUTED },
            width,
        );
    }
    if speed.is_some_and(|(_, state)| state == "LAST") {
        append_detail(
            &mut line,
            telemetry_age(entry.output_speed.map(|speed| speed.observed_at)),
            MUTED,
            width,
        );
    }
    line
}

fn draw_bar(
    frame: &mut Frame,
    area: Rect,
    usage: &domain::RequestUsage,
    color: Color,
    ceiling: u64,
) {
    let queued = usage.queued;
    if area.is_empty() {
        return;
    }
    // Cache metadata must not change a bar's height. Only a full cell can
    // contain two segment colors without painting above the measured value.
    let total = (u128::from(usage.prompt.min(ceiling)) * u128::from(area.height) * 8)
        .div_ceil(u128::from(ceiling.max(1))) as u64;
    let cached = usage
        .cached
        .filter(|n| *n <= usage.prompt)
        .filter(|_| usage.prompt > 0)
        .map(|n| (u128::from(n) * u128::from(total) / u128::from(usage.prompt)) as u64)
        .unwrap_or(0);
    let blocks = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    if total == 0 {
        frame.buffer_mut()[(area.x, area.bottom() - 1)]
            .set_symbol("·")
            .set_fg(DIM);
    }
    for row in 0..area.height {
        let part = total.saturating_sub(u64::from(row) * 8).min(8) as usize;
        if part == 0 {
            continue;
        }
        let cached_part = cached.saturating_sub(u64::from(row) * 8).min(8) as usize;
        // Full cells are filled backgrounds: terminals leave a gap between
        // rows of block glyphs. A queued request is hatched: not running yet.
        let (glyph, fg, bg) = if queued {
            (if part == 8 { "░" } else { blocks[part] }, color, PANEL)
        } else if cached_part == part && part == 8 {
            (" ", GREEN, GREEN)
        } else if cached_part == part {
            (blocks[part], GREEN, PANEL)
        } else if cached_part > 0 && part == 8 {
            (blocks[cached_part], GREEN, color)
        } else if part == 8 {
            (" ", color, color)
        } else {
            // A partial cell has an empty area too: keep its measured height
            // and leave a mixed cell unsplit, so equal cache ratios never flip
            // color with rounding. The readout retains exact reuse.
            (blocks[part], color, PANEL)
        };
        frame.buffer_mut()[(area.x, area.bottom() - 1 - row)]
            .set_symbol(glyph)
            .set_fg(fg)
            .set_bg(bg);
    }
    if usage.prompt > ceiling {
        frame.buffer_mut()[(area.x, area.y)]
            .set_symbol("↑")
            .set_fg(color);
    }
}

pub(super) fn draw(
    frame: &mut Frame,
    area: Rect,
    history: &History,
    sample: &Sample,
    scroll: usize,
    zoom: u16,
) {
    let selected = history
        .len()
        .checked_sub(1)
        .map(|last| last - scroll.min(last));
    let window =
        selected.map(|index| history_window(history, index, area.width.saturating_sub(2), zoom));
    let ceiling = chart_scale::ceiling(
        window
            .map(|(start, visible, _)| {
                history
                    .entries
                    .iter()
                    .skip(start)
                    .take(visible)
                    .map(|entry| entry.usage.prompt)
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0),
        1,
    );
    let now = SystemTime::now();
    let live = selected.is_some_and(|i| is_live(&history.entries[i], sample, now));
    let cached = selected.and_then(|i| {
        let usage = &history.entries[i].usage;
        usage.cached.filter(|n| *n <= usage.prompt)
    });
    let mut title = Line::from(vec![
        Span::styled(
            " prompt load ",
            Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if selected.is_some() {
                format!("· 0–{} tokens", exact(ceiling))
            } else {
                String::new()
            },
            Style::default().fg(MUTED),
        ),
    ]);
    title.spans.push(Span::raw(" "));
    let mut block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM))
        .style(Style::default().bg(PANEL));
    if let Some(index) = selected {
        block = block.title_bottom(size_summary(history, index, area.width.saturating_sub(2)));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }
    let Some(index) = selected else {
        // Nothing to chart yet: one line says why, and the Overview gives
        // this panel no more rows than that line needs.
        let line = if sample.llm_status == "untracked" {
            "busy outside oMLX · requests sent straight to Splash report no prompt counts"
        } else if sample.llm_active_requests.is_some_and(|n| n > 0) {
            "waiting for the prompt size"
        } else {
            "no requests yet"
        };
        frame.render_widget(
            Paragraph::new(line)
                .alignment(Alignment::Center)
                .style(Style::default().fg(MUTED)),
            Rect::new(
                inner.x,
                inner.y + inner.height.saturating_sub(1) / 2,
                inner.width,
                1,
            ),
        );
        return;
    };
    let entry = &history.entries[index];
    let previous = index.checked_sub(1).and_then(|i| history.entries.get(i));
    let color = if live { CYAN } else { BLUE };
    let state = if live && entry.usage.queued {
        "QUEUED"
    } else if live {
        "LIVE"
    } else if entry.usage.completed {
        "REPORTED"
    } else {
        "LAST SEEN"
    };
    let cache_label = cached
        .map(|n| {
            let ratio = if entry.usage.prompt == 0 {
                0.0
            } else {
                n as f64 / entry.usage.prompt as f64
            };
            format!("CACHED {:.0}%", ratio * 100.0)
        })
        .unwrap_or_else(|| "CACHED —".into());
    // The cache chart's bands; reuse barely matters for small prompts.
    let cache_color = match cached {
        None => MUTED,
        Some(_) if entry.usage.prompt < CACHE_GRADED_PROMPT => CYAN,
        Some(n) => ChartMetric::Cache
            .tone(
                (u128::from(n) * 100 / u128::from(entry.usage.prompt)) as u64,
                Thresholds::default(),
            )
            .color(),
    };
    let cache_span = Span::styled(cache_label, Style::default().fg(cache_color));
    let mut headline = Line::from(vec![
        Span::styled(
            format!("{} tokens", exact(entry.usage.prompt)),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" · {state}"), Style::default().fg(color)),
    ]);
    // Cache and observation age outrank the clock on narrow primary panels.
    // Reserve cache space before adding optional metadata to the headline.
    let headline_width = inner.width.saturating_sub(cache_span.width() as u16 + 2);
    // A live request is happening now: its age and clock say nothing new.
    let timestamp = clock_stamp(entry.last_seen, sample.utc_offset);
    let mut clock_in_header = live;
    if !live {
        append_detail(
            &mut headline,
            telemetry_age(Some(entry.last_seen)),
            MUTED,
            headline_width,
        );
        let before_clock = headline.width();
        append_detail(&mut headline, timestamp.clone(), MUTED, headline_width);
        clock_in_header = headline.width() > before_clock;
    }
    let cache_in_header = headline.width() + cache_span.width() + 2 <= usize::from(inner.width);
    if cache_in_header {
        headline.spans.push(Span::raw(
            " ".repeat(usize::from(inner.width) - headline.width() - cache_span.width()),
        ));
        headline.spans.push(cache_span.clone());
    }
    frame.render_widget(
        Paragraph::new(headline),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    if inner.height < 2 {
        return;
    }
    // History survives runtime/model changes. Attribute the selected request
    // when SYSINFO describes a different runtime, without taking a plot row
    // away from its size labels. Exact count, cache and age retain priority.
    let other_runtime =
        entry.usage.provider != sample.llm_provider || entry.usage.model != sample.llm_model;
    let comparison = compact_comparison(&entry.usage, previous);
    let mut details = if other_runtime {
        let cache_width = if cache_in_header {
            0
        } else {
            cache_span.width() + 3
        };
        Line::from(Span::styled(
            compact_label(
                &format!("REQUEST {} · {}", entry.usage.provider, entry.usage.model),
                usize::from(inner.width).saturating_sub(cache_width),
            ),
            Style::default().fg(color),
        ))
    } else {
        Line::from(Span::styled(comparison.clone(), Style::default().fg(MUTED)))
    };
    if !cache_in_header {
        append_detail(
            &mut details,
            cache_span.content.into_owned(),
            cache_color,
            inner.width,
        );
    }
    if other_runtime {
        append_detail(&mut details, comparison, MUTED, inner.width);
    }
    if let Some(cached) = cached {
        append_detail(
            &mut details,
            format!("{} uncached", exact(entry.usage.prompt - cached)),
            color,
            inner.width,
        );
    }
    // Position in the history only matters while browsing older requests.
    if index + 1 < history.len() {
        append_detail(
            &mut details,
            format!("#{} / {}", entry.number, history.next_number),
            MUTED,
            inner.width,
        );
    }
    frame.render_widget(
        Paragraph::new(if inner.height >= 5 {
            details
        } else {
            let mut output = progress_or_output(entry, live, sample, inner.width);
            if other_runtime {
                append_detail(
                    &mut output,
                    format!("REQUEST {} · {}", entry.usage.provider, entry.usage.model),
                    color,
                    inner.width,
                );
            }
            output
        }),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );
    let metadata_height = if inner.height >= 5 {
        let mut output = progress_or_output(entry, live, sample, inner.width);
        if !clock_in_header {
            append_detail(&mut output, timestamp, MUTED, inner.width);
            append_detail(
                &mut output,
                telemetry_age(Some(entry.last_seen)),
                MUTED,
                inner.width,
            );
        }
        frame.render_widget(
            Paragraph::new(output),
            Rect::new(inner.x, inner.y + 2, inner.width, 1),
        );
        3
    } else {
        2
    };
    let (start, visible, slot_width) = window.unwrap();
    // The prompt size sets the bar's height; the tokens generated so far go
    // under it when the plot keeps enough rows without it. That depends on the
    // panel's height alone, so the bars never shrink when output first
    // arrives. Size labels outrank it.
    let show_outputs = inner.height >= metadata_height + 2 + OUTPUT_ROW_MIN_PLOT;
    let footer = 1 + u16::from(show_outputs);
    if inner.height < metadata_height + footer + 1 {
        return;
    }
    // Reserve enough horizontal space for every bar's size, including the
    // selection marker. History remains ordinal, with new requests at right.
    let plot = Rect::new(
        inner.x,
        inner.y + metadata_height,
        inner.width,
        inner.height - metadata_height - footer,
    );
    let first_x = plot.right() - visible as u16 * slot_width;
    for (offset, old) in history.entries.iter().skip(start).take(visible).enumerate() {
        let x = first_x + offset as u16 * slot_width;
        let color = if is_live(old, sample, now) {
            CYAN
        } else {
            BLUE
        };
        let bar_width = slot_width
            .saturating_sub(2)
            .min(12_u16.saturating_mul(zoom.max(1)))
            .max(1);
        let bar_x = x + slot_width.saturating_sub(bar_width) / 2;
        for column in 0..bar_width {
            draw_bar(
                frame,
                Rect::new(bar_x + column, plot.y, 1, plot.height),
                &old.usage,
                color,
                ceiling,
            );
        }
        let selected = start + offset == index;
        frame.render_widget(
            Paragraph::new(format!(
                "{}{}",
                compact_tokens(old.usage.prompt),
                if selected { "▲" } else { "" }
            ))
            .alignment(Alignment::Center)
            .style(if selected {
                Style::default().fg(color).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(MUTED)
            }),
            Rect::new(x, plot.bottom(), slot_width, 1),
        );
        if show_outputs {
            frame.render_widget(
                Paragraph::new(output_label(&old.usage, slot_width))
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(MUTED)),
                Rect::new(x, plot.bottom() + 1, slot_width, 1),
            );
        }
    }
}

#[cfg(test)]
#[path = "tests/request_dashboard.rs"]
mod tests;
