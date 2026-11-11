// SPDX-License-Identifier: MIT
//! What each Overview panel says beside its plot: the reading in its title,
//! the line on its bottom edge, and the rows some panels draw above the
//! plot (memory headroom, the KV cache's bars, compression's ratio, swap).
//! The plot itself is drawn by `render_indicator_chart` in `ui.rs`.
use crate::app::App;
use crate::cache_impact;
use crate::chart_navigation::Chart;
use crate::chart_render::chart_inactive_rate_label;
use crate::config::PAGING_ACTIVE_ENTER_RATE;
use crate::domain::{ChartMetric, ChartPoint, Tone};
use crate::formatting::*;
use crate::swap_usage;
use crate::theme::*;
use crate::ui::*;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::collections::VecDeque;

impl App {
    /// The reading in a panel's title: what its newest sample says.
    pub(crate) fn chart_reading(
        &self,
        area: Rect,
        history: &VecDeque<ChartPoint>,
        metric: ChartMetric,
    ) -> String {
        let sample = &self.collector.current;
        let current = history.back().and_then(|point| point.value);
        match metric {
            // Drop qualifiers before units: a bare number is never the reading.
            ChartMetric::Generation | ChartMetric::Prefill => current
                .map(|value| {
                    let rate = format!("{:.1} tok/s", value as f64 / 10.0);
                    if area.width < 38 {
                        rate
                    } else {
                        let live = match metric {
                            ChartMetric::Generation => sample.llm_generation_tps_live,
                            ChartMetric::Prefill => sample.llm_prefill_tps_live,
                            _ => false,
                        };
                        if live {
                            format!("LIVE {rate}")
                        } else {
                            format!("LAST {rate}")
                        }
                    }
                })
                .unwrap_or_else(|| {
                    // The title reads what the chart's last bar shows. A
                    // server average beside a different plotted sample
                    // contradicts it; the average goes to the bottom border.
                    if let Some(point) = history
                        .iter()
                        .filter(|_| area.width >= 38)
                        .rev()
                        .take(self.charts.visible_samples(
                            Chart::from(metric),
                            usize::from(area.width.saturating_sub(8)),
                        ))
                        .find(|point| point.value.is_some())
                    {
                        let rate =
                            format!("{:.1} tok/s", point.value.unwrap_or_default() as f64 / 10.0);
                        let with_age =
                            format!("LAST {rate} · {}", telemetry_age(Some(point.observed_at)));
                        // Narrow panels keep the state word (decoding), which
                        // says why there is no current rate.
                        if area.width >= 44 {
                            return with_age;
                        } else if area.width >= 38 {
                            return format!("LAST {rate}");
                        }
                    }
                    if metric == ChartMetric::Prefill
                        && sample.llm_prefill_tps.is_some()
                        && area.width >= 38
                    {
                        return llm_prefill_rate_label(sample);
                    }
                    let label = chart_inactive_rate_label(metric, &self.collector.current);
                    if area.width < 38 {
                        match label.as_str() {
                            "decode active" => "decoding".into(),
                            "prefill active" => "prefilling".into(),
                            _ => label,
                        }
                    } else {
                        label
                    }
                }),
            ChartMetric::Cache => {
                // Hit rate is measured only over intervals that read new
                // prompt tokens; between prompts, keep the last one, marked.
                let last = history
                    .iter()
                    .rev()
                    .take(self.charts.visible_samples(
                        Chart::from(metric),
                        usize::from(area.width.saturating_sub(8)),
                    ))
                    .find_map(|point| point.value.map(|value| (value, point.observed_at)));
                let value = match (current, last) {
                    (Some(value), _) => format!("{value}%"),
                    (None, Some((value, at))) if area.width >= 44 => {
                        format!("LAST {value}% · {}", telemetry_age(Some(at)))
                    }
                    (None, Some((value, _))) if area.width >= 28 => format!("LAST {value}%"),
                    _ => "—".into(),
                };
                if value.starts_with("LAST") || area.width < 28 {
                    value
                } else {
                    format!("hit {value}")
                }
            }
            ChartMetric::Gpu => current
                .map(|value| format!("{value}%"))
                .unwrap_or_else(|| "—".into()),
            // OS pressure is the memory verdict; resident occupancy (which
            // includes file cache) is the supporting reading beneath it.
            ChartMetric::Memory => {
                let state = pressure_state_label(&self.collector.current);
                if area.width >= 34 {
                    format!("PRESSURE {state}")
                } else {
                    state.into()
                }
            }
            ChartMetric::Swap | ChartMetric::Compression => {
                current.map(rate).unwrap_or_else(|| "—".into())
            }
            // What the cache is worth, not how fast the disk moved.
            ChartMetric::Disk => cache_impact::headline(
                sample,
                cache_impact::prefill_speed(&self.collector.prefill_history, sample),
                area.width,
            ),
        }
    }

    /// The line on a panel's bottom edge, when it has one.
    pub(crate) fn chart_summary(
        &self,
        area: Rect,
        history: &VecDeque<ChartPoint>,
        metric: ChartMetric,
    ) -> Option<String> {
        let sample = &self.collector.current;
        let insight = &sample.correlation;
        let current = history.back().and_then(|point| point.value);
        match metric {
            ChartMetric::Generation
                if self.charts.expanded
                    && sample.llm_generation_tps.is_some()
                    && !sample.llm_generation_tps_live =>
            {
                Some(llm_generation_rate_label(sample))
            }
            ChartMetric::Prefill
                if self.charts.expanded
                    && sample.llm_prefill_tps.is_some()
                    && !sample.llm_prefill_tps_live =>
            {
                Some(llm_prefill_rate_label(sample))
            }
            // While a prefill runs, its position replaces the unit: the rate is already in the title,
            // and 40.9k / 61.5k tells you how long the wait is.
            ChartMetric::Prefill => {
                Some(llm_prefill_progress_label(sample).unwrap_or_else(|| "tok/s".into()))
            }
            // oMLX and Splash report generation summed over running requests;
            // say so when there are several, since each gets only its share.
            ChartMetric::Generation => {
                let mut unit = match generating_requests(sample) {
                    n if n > 1 && area.width >= 38 => format!("tok/s · total of {n} requests"),
                    n if n > 1 => format!("tok/s · {n} req"),
                    _ => "tok/s".into(),
                };
                // What speculative decoding earns: often the largest reason
                // one answer runs faster than another on a healthy machine.
                if let Some(share) = sample.llm_draft_acceptance {
                    unit.push_str(&format!(" · draft {:.0}%", share * 100.0));
                }
                // What the dashed line is: the speed slowdowns are judged
                // against. It gives way before the unit does.
                // With the work it is for, when it is for one kind of work.
                let usual = insight
                    .usual_speed
                    .filter(|_| insight.concerns(metric))
                    .and_then(|(usual, _)| {
                        let mut texts = Vec::new();
                        if !insight.usual_for.is_empty() {
                            texts.push(format!(
                                "{unit} · ┄ usual {usual:.1} for {}",
                                insight.usual_for
                            ));
                        }
                        texts.push(format!("{unit} · ┄ usual {usual:.1}"));
                        texts.into_iter().find(|text| {
                            Line::from(text.as_str()).width() + 4 <= usize::from(area.width)
                        })
                    });
                Some(usual.unwrap_or(unit))
            }
            ChartMetric::Memory => None,
            ChartMetric::Gpu => Some(if current == Some(0) {
                "idle".into()
            } else {
                gpu_load_label(sample.gpu_util, self.thresholds).into()
            }),
            // A short panel's title already has the rate; the split would
            // only repeat it.
            ChartMetric::Compression if area.height < 7 => None,
            ChartMetric::Compression => {
                let (compress, decompress) = if sample.rate_ready && sample.vm_available {
                    (rate(sample.compress), rate(sample.decompress))
                } else {
                    ("—".into(), "—".into())
                };
                // Narrow panels share one unit instead of abbreviating names.
                let shared_unit = compress
                    .rsplit_once(' ')
                    .zip(decompress.rsplit_once(' '))
                    .filter(|((_, a), (_, b))| a == b)
                    .map(|((c, unit), (d, _))| {
                        format!("compressing {c} · decompressing {d} {unit}")
                    });
                let fits = |text: &String| {
                    Line::from(text.as_str()).width() + 2
                        <= usize::from(area.width.saturating_sub(2))
                };
                [
                    Some(format!(
                        "compressing {compress} · decompressing {decompress}"
                    )),
                    shared_unit,
                    compress
                        .rsplit_once(' ')
                        .zip(decompress.rsplit_once(' '))
                        .filter(|((_, a), (_, b))| a == b)
                        .map(|((c, unit), (d, _))| format!("comp {c} · decomp {d} {unit}")),
                    Some(format!("comp {compress} · decomp {decompress}")),
                ]
                .into_iter()
                .flatten()
                .find(fits)
                .or_else(|| Some(format!("comp {compress}")))
            }
            // What the paging means for the model, in words; the title has
            // the rate and the chart how it moved.
            ChartMetric::Swap => {
                if !sample.rate_ready || sample.paging_unavailable {
                    return Some("paging unavailable".into());
                }
                let swapping = sample.swap_in.saturating_add(sample.swap_out) > 0;
                let files = sample.page_in.saturating_add(sample.page_out) > 0;
                Some(
                    match (swapping, files, sample.memory_tight()) {
                        (true, _, _) => "swapping: memory is short",
                        (false, true, true) => "reading files back: memory is tight",
                        (false, true, false) => "reading files · memory not tight",
                        (false, false, _) => "no paging",
                    }
                    .into(),
                )
            }
            // What the SSD tier is doing, in words; the chart shows how much.
            ChartMetric::Disk => cache_impact::activity(sample).map(|(state, _)| state.to_owned()),
            // The title has the latest interval's hit rate; these are the
            // session-wide figures it is measured against.
            ChartMetric::Cache => Some(if area.width < 38 {
                format!("since start {}", percent(sample.llm_cache_efficiency))
            } else {
                format!(
                    "since start {} · prefix {}",
                    percent(sample.llm_cache_efficiency),
                    percent(sample.llm_prefix_hit_rate)
                )
            }),
        }
    }

    /// A panel's rows above its plot. Returns true when the panel has
    /// nothing more to draw.
    pub(crate) fn draw_panel_rows(
        &self,
        frame: &mut Frame,
        area: Rect,
        inner: &mut Rect,
        history: &VecDeque<ChartPoint>,
        metric: ChartMetric,
        current_tone: Tone,
    ) -> bool {
        let sample = &self.collector.current;
        let current = history.back().and_then(|point| point.value);
        if metric == ChartMetric::Memory {
            // Headroom first: wired against the GPU limit, RAM against what
            // is left, and what was left over time. Without the OS's
            // composition counters, the plain occupancy reading below.
            let visible = |width: usize| self.charts.visible_samples(Chart::Memory, width);
            let columns = |width: usize| {
                let tones = &self.collector.load_history;
                let lefts = self.composition_columns(visible(width), width);
                let shown = visible(width);
                let start = tones.len().saturating_sub(shown);
                let padding = shown - (tones.len() - start);
                lefts
                    .into_iter()
                    .enumerate()
                    .map(|(column, sizes)| {
                        let tone = (column * shown / width)
                            .checked_sub(padding)
                            .and_then(|offset| tones.get(start + offset))
                            .map_or(Tone::Green, |point| point.tone);
                        sizes.map(|sizes| (crate::memory_headroom::left(sizes), tone))
                    })
                    .collect()
            };
            if crate::memory_headroom::draw(frame, *inner, sample, columns) {
                return true;
            }
            let usage = match (sample.resident_memory, current) {
                (Some(used), _) => format!("{} / {}", bytes(used), bytes(sample.total_memory)),
                (None, Some(_)) if sample.total_memory > 0 => {
                    format!("RAM {}", bytes(sample.total_memory))
                }
                (None, Some(_)) => "RAM —".into(),
                (None, None) => "RAM reading unavailable".into(),
            };
            // Keep the occupancy percentage on narrow panels by sharing the unit.
            let compact_usage = sample.resident_memory.and_then(|used| {
                let (used, unit) = bytes(used)
                    .rsplit_once(' ')
                    .map(|(n, u)| (n.to_owned(), u.to_owned()))?;
                let (total, total_unit) = bytes(sample.total_memory)
                    .rsplit_once(' ')
                    .map(|(n, u)| (n.to_owned(), u.to_owned()))?;
                (unit == total_unit).then(|| format!("{used}/{total} {unit}"))
            });
            let mut line = Line::from(Span::styled(usage.clone(), Style::default().fg(MUTED)));
            if let Some(value) = current {
                let candidates = [
                    (usage, format!(" · {value}% resident")),
                    (
                        compact_usage.clone().unwrap_or_default(),
                        format!(" · {value}% resident"),
                    ),
                    (compact_usage.unwrap_or_default(), format!(" · {value}%")),
                ];
                if let Some((text, resident)) = candidates.into_iter().find(|(text, resident)| {
                    !text.is_empty()
                        && Line::from(format!("{text}{resident}").as_str()).width()
                            <= usize::from(inner.width)
                }) {
                    line = Line::from(vec![
                        Span::styled(text, Style::default().fg(MUTED)),
                        Span::styled(resident, Style::default().fg(CYAN)),
                    ]);
                }
            }
            frame.render_widget(
                Paragraph::new(line),
                Rect::new(inner.x, inner.y, inner.width, 1),
            );
            if inner.height < 2 {
                return true;
            }
            inner.y += 1;
            inner.height -= 1;
            // Short panels cannot hold a meaningful trace: the exact reading
            // replaces a flat line against a 0–100% axis.
            if inner.height < 3 {
                return true;
            }
        }
        if metric == ChartMetric::Compression {
            if inner.height >= 5 && sample.compressor > 0 && sample.compressed_logical > 0 {
                let (stored, held) = (bytes(sample.compressed_logical), bytes(sample.compressor));
                let ratio = sample.compressed_logical as f64 / sample.compressor as f64;
                let text = [
                    format!("{stored} stored in {held} · {ratio:.1}× ratio"),
                    format!("{stored} in {held} · {ratio:.1}×"),
                    format!("{ratio:.1}× ratio"),
                ]
                .into_iter()
                .find(|text| Line::from(text.as_str()).width() <= usize::from(inner.width))
                .unwrap_or_default();
                frame.render_widget(
                    Paragraph::new(text).style(Style::default().fg(MUTED)),
                    Rect::new(inner.x, inner.y, inner.width, 1),
                );
                inner.y += 1;
                inner.height -= 1;
            }
            if inner.is_empty() {
                return true;
            }
            if inner.height < 3 {
                // Too short for an axis: the traffic as a sparkline; its rate
                // and colour are in the title, never a sentence.
                self.draw_sparkline(frame, *inner, history, metric);
                return true;
            }
        }
        if metric == ChartMetric::Disk {
            // The share of prompt tokens the cache served, over the session and
            // for the last prompt, each with the prefill time that saved. Each
            // bar needs a row of chart and the SSD row left beneath it.
            let speed = cache_impact::prefill_speed(&self.collector.prefill_history, sample);
            let reserved = if sample.ssd_cache.is_some() { 2 } else { 1 };
            // Weights read while decoding come first: they cost every token.
            if let Some(weights) = cache_impact::weights(&self.collector.decode_reads) {
                if inner.height > reserved {
                    cache_impact::draw_weights(
                        frame,
                        Rect::new(inner.x, inner.y, inner.width, 1),
                        weights,
                    );
                    inner.y += 1;
                    inner.height -= 1;
                }
            }
            // Where the session's prompt tokens came from: memory, the SSD
            // or computed. Without the tier's own counts, its reused share.
            let tiers = cache_impact::tiers(sample);
            if let Some(tiers) = tiers {
                if inner.height > reserved {
                    cache_impact::draw_tiers(
                        frame,
                        Rect::new(inner.x, inner.y, inner.width, 1),
                        tiers,
                    );
                    inner.y += 1;
                    inner.height -= 1;
                }
            }
            let session = cache_impact::session(sample).filter(|_| tiers.is_none());
            let bars = [
                session.map(|reuse| ("session", reuse)),
                cache_impact::last_prompt(sample).map(|reuse| ("last", reuse)),
            ];
            for (label, reuse) in bars.into_iter().flatten() {
                if inner.height <= reserved {
                    break;
                }
                // The runtime's lifetime share has no saved time beside it:
                // the title counts only what mlxtop watched.
                cache_impact::draw_reuse(
                    frame,
                    Rect::new(inner.x, inner.y, inner.width, 1),
                    label,
                    reuse,
                    speed.filter(|_| label == "last"),
                );
                inner.y += 1;
                inner.height -= 1;
            }
            if sample.ssd_cache.is_some() {
                swap_usage::draw_ssd_cache(
                    frame,
                    Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
                    sample,
                );
                inner.height = inner.height.saturating_sub(1);
            }
            if inner.is_empty() {
                return true;
            }
            if inner.height < 3 {
                // Too short for an axis: the traffic as a sparkline, never a sentence.
                self.draw_sparkline(frame, *inner, history, metric);
                return true;
            }
        }
        if metric == ChartMetric::Swap {
            swap_usage::draw(
                frame,
                Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
                sample,
            );
            inner.height = inner.height.saturating_sub(1);
            if inner.is_empty() {
                return true;
            }
            if inner.height < 3 {
                // Graded on what counts against the system, as the assessment
                // is: file paging only while memory is tight.
                let uncounted = if sample.memory_tight() {
                    0
                } else {
                    sample.page_in.saturating_add(sample.page_out)
                };
                let counted = current.map(|rate| rate.saturating_sub(uncounted));
                frame.render_widget(
                    // Match the assessment: "active" starts at the warning rate.
                    Paragraph::new(match (current, counted) {
                        (Some(0), _) => "No paging traffic",
                        (_, Some(rate)) if rate >= PAGING_ACTIVE_ENTER_RATE => "Paging active",
                        (_, Some(rate)) if rate >= self.thresholds.swap_warn_rate => "Watch paging",
                        (_, Some(0)) => "Disk reads · memory not tight",
                        (Some(_), _) => "Light paging · below warning",
                        (None, _) => "Paging unavailable",
                    })
                    .style(Style::default().fg(current_tone.color())),
                    *inner,
                );
                return true;
            }
        }
        if metric == ChartMetric::Cache && area.width < 38 && inner.height >= 4 {
            frame.render_widget(
                Paragraph::new(format!(
                    "PREFIX HIT {}",
                    percent(sample.llm_prefix_hit_rate)
                ))
                .style(Style::default().fg(MUTED)),
                Rect::new(inner.x, inner.y, inner.width, 1),
            );
            inner.y += 1;
            inner.height -= 1;
        }
        false
    }
}
