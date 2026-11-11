// SPDX-License-Identifier: MIT
//! A dense system strip: runtime identity, workload and host facts share one border.
use crate::chart_render::meter_cells;
use crate::domain::{CorrelationCause, Sample, Tone};
use crate::formatting::{
    activity_frame, bytes, compact_label, compact_tokens, compressed_memory_label,
    llm_context_label, llm_is_working, llm_runtime_label, optional_tokens, process_count_label,
    signed_rate, telemetry_age, telemetry_source,
};
use crate::theme::{
    card_block, llm_status_tone, tone_badge, BLUE, CYAN, DIM, EDGE, MUTED, RED, YELLOW,
};
use crate::{gpu, request_history};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::env;

fn fitted_parts(parts: impl IntoIterator<Item = String>, width: usize) -> String {
    let mut text = String::new();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        let next = if text.is_empty() {
            part.clone()
        } else {
            format!("{text} · {part}")
        };
        if Line::from(next.as_str()).width() <= width {
            text = next;
        } else if text.is_empty() {
            text = compact_label(&part, width);
        }
    }
    text
}

fn work_parts(sample: &Sample, history: &request_history::History) -> Vec<String> {
    if sample.llm_prompt_tokens.is_none() && sample.llm_active_requests == Some(0) {
        if let Some((request, observed)) = history.latest_for(sample) {
            return vec![
                format!("LAST PROMPT {}", compact_tokens(request.prompt)),
                telemetry_age(Some(observed)),
                format!("OUT {}", optional_tokens(request.output)),
            ];
        }
    }
    vec![
        format!("PROMPT {}", optional_tokens(sample.llm_prompt_tokens)),
        format!("OUT {}", optional_tokens(sample.llm_output_tokens)),
        format!("CONTEXT {}", llm_context_label(sample)),
    ]
}

/// Overview's prompt load panel already shows the latest request's prompt and
/// output. Keep runtime counters here only when they add a different reading.
fn prompt_panel_covers(sample: &Sample, history: &request_history::History) -> bool {
    history.latest_for(sample).is_some_and(|(request, _)| {
        sample
            .llm_prompt_tokens
            .is_none_or(|prompt| prompt == request.prompt)
    })
}

fn hardware_parts(sample: &Sample) -> Vec<String> {
    if sample.has_discrete_gpus() {
        return vec![if sample.gpus.len() == 1 {
            sample.gpus[0].name.clone()
        } else {
            format!("{} {} GPUs", sample.gpus.len(), gpu::vendors(&sample.gpus))
        }];
    }
    let mut parts = vec![sample
        .metal
        .device_name
        .clone()
        .unwrap_or_else(|| env::consts::OS.into())];
    if let Some(cores) = sample.metal.gpu_cores {
        parts.push(format!("{cores} GPU cores"));
    }
    parts
}

fn gpu_memory(sample: &Sample) -> Option<String> {
    if sample.has_discrete_gpus() {
        return gpu::memory_totals(&sample.gpus)
            .map(|(used, total)| format!("VRAM sum {} / {}", bytes(used), bytes(total)));
    }
    match (sample.gpu_in_use, sample.gpu_alloc) {
        (Some(used), Some(total)) => Some(format!("GPU mem {} / {}", bytes(used), bytes(total))),
        (Some(used), None) => Some(format!("GPU mem {}", bytes(used))),
        (None, Some(total)) => Some(format!("GPU mem — / {}", bytes(total))),
        (None, None) => None,
    }
}

fn ram(sample: &Sample) -> String {
    format!(
        "RAM {}",
        if sample.total_memory > 0 {
            bytes(sample.total_memory)
        } else {
            "—".into()
        }
    )
}

fn cpu(sample: &Sample) -> String {
    if sample.llm_count > 0 {
        format!("CPU {:.1}%", sample.llm_cpu)
    } else {
        "CPU —".into()
    }
}

fn rss(sample: &Sample) -> String {
    format!(
        "LLM RSS {}",
        if sample.llm_count > 0 {
            bytes(sample.llm_rss)
        } else {
            "—".into()
        }
    )
}

/// Thermal state only when it is news: normal is the default.
fn thermal_note(sample: &Sample) -> Option<String> {
    (!matches!(
        sample.thermal.as_str(),
        "no warning" | "nominal" | "unavailable"
    ))
    .then(|| format!("THERMAL {}", sample.thermal))
}

fn thermal_parts(sample: &Sample) -> Vec<String> {
    let mut parts: Vec<String> = thermal_note(sample).into_iter().collect();
    if let Some(renderer) = sample.metal.renderer_util {
        parts.push(format!("R {renderer}%"));
    }
    if let Some(tiler) = sample.metal.tiler_util {
        parts.push(format!("T {tiler}%"));
    }
    parts
}

fn plain(value: String, color: Color) -> Line<'static> {
    Line::from(Span::styled(value, Style::default().fg(color)))
}

fn identity(sample: &Sample, width: usize) -> Line<'static> {
    Line::from(Span::styled(
        compact_label(
            &format!("{} · {}", llm_runtime_label(sample), sample.llm_model),
            width,
        ),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    ))
}

fn state(sample: &Sample) -> Line<'static> {
    // Prompt load shows prefill progress with its time left; the badge
    // names the state only.
    let status = sample.llm_status.to_ascii_uppercase();
    let badge = if llm_is_working(&sample.llm_status) {
        format!("{} {status}", activity_frame(sample))
    } else {
        status
    };
    let mut line = Line::from(tone_badge(llm_status_tone(&sample.llm_status), &badge));
    let source = telemetry_source(sample);
    if !source.is_empty() {
        line.spans.push(Span::styled(
            format!("  {source}"),
            Style::default().fg(MUTED),
        ));
    }
    line
}

/// GPU memory against its limit as a small meter, graded as it nears full:
/// the Metal working set filling up is what slows generation.
fn gpu_memory_meter(sample: &Sample) -> Option<Vec<Span<'static>>> {
    let (used, total) = if sample.has_discrete_gpus() {
        gpu::memory_totals(&sample.gpus)?
    } else {
        (sample.gpu_in_use?, sample.gpu_alloc?)
    };
    if total == 0 {
        return None;
    }
    let percent = used as f64 * 100.0 / total as f64;
    let color = if percent >= 97.0 {
        RED
    } else if percent >= 90.0 {
        YELLOW
    } else {
        CYAN
    };
    Some(vec![
        Span::styled(
            if sample.has_discrete_gpus() {
                "VRAM "
            } else {
                "GPU mem "
            },
            Style::default().fg(MUTED),
        ),
        Span::styled(meter_cells(percent, 5), Style::default().fg(color).bg(EDGE)),
        Span::styled(
            format!(" {} / {}", bytes(used), bytes(total)),
            Style::default().fg(color),
        ),
    ])
}

/// The card's bottom edge: the machine, then what the runtime uses of it.
/// Whole fields fit or drop, least important last; thermal only when it is
/// not normal.
fn host_footer(sample: &Sample, width: usize) -> Line<'static> {
    let muted = |text: String| vec![Span::styled(text, Style::default().fg(MUTED))];
    let mut fields: Vec<Vec<Span<'static>>> = vec![muted(hardware_parts(sample).join(" · "))];
    // A reading that is unusual takes the anomaly colour here, where it is
    // shown, with its usual value.
    let insight = &sample.correlation;
    if let Some(thermal) = thermal_note(sample) {
        let color = if insight.note(CorrelationCause::Thermal).is_some() {
            Tone::Unusual.color()
        } else {
            YELLOW
        };
        fields.push(vec![Span::styled(thermal, Style::default().fg(color))]);
    }
    fields.extend(gpu_memory_meter(sample));
    if sample.llm_count > 0 {
        let mut field = muted(format!("LLM {} · ", bytes(sample.llm_rss)));
        field.push(match insight.note(CorrelationCause::Cpu) {
            Some(note) => Span::styled(
                note.trim_start_matches("runtime ").to_owned(),
                Style::default().fg(Tone::Unusual.color()),
            ),
            None => Span::styled(
                format!("CPU {:.0}%", sample.llm_cpu),
                Style::default().fg(MUTED),
            ),
        });
        fields.push(field);
    }
    let mut line = Line::default();
    for field in fields {
        let separator = usize::from(!line.spans.is_empty()) * 3;
        let field_width: usize = field.iter().map(Span::width).sum();
        if line.width() + separator + field_width > width {
            continue;
        }
        if separator > 0 {
            line.spans
                .push(Span::styled(" · ", Style::default().fg(DIM)));
        }
        line.spans.extend(field);
    }
    line
}

pub(super) fn draw(
    frame: &mut Frame,
    area: Rect,
    sample: &Sample,
    history: &request_history::History,
) {
    let width = usize::from(area.width.saturating_sub(2));
    let compact = area.height < 5;
    let wide = width >= 130;
    let tone = llm_status_tone(&sample.llm_status);
    // Work counters only when they have values the prompt panel lacks.
    let title = fitted_parts(
        if compact && !prompt_panel_covers(sample, history) {
            work_parts(sample, history)
                .into_iter()
                .filter(|part| !part.ends_with('—'))
                .collect()
        } else {
            Vec::new()
        },
        width.saturating_sub(2),
    );
    let footer = if compact {
        String::new()
    } else if wide {
        let mut parts = Vec::new();
        if let Some(memory) = &sample.process_memory {
            parts.push(format!("OS peak {}", bytes(memory.peak)));
            if let Some(growth) = sample.process_memory_growth {
                parts.push(format!("growth {}", signed_rate(growth)));
            }
        }
        if let Some(version) = &sample.mlx.version {
            parts.push(format!("MLX {version}"));
        }
        fitted_parts(parts, width.saturating_sub(2))
    } else {
        fitted_parts(
            gpu_memory(sample)
                .into_iter()
                .chain([format!("COMP {}", compressed_memory_label(sample))])
                .chain(
                    (sample.llm_count > 0)
                        .then(|| format!("LOCAL {}", process_count_label(sample))),
                ),
            width.saturating_sub(2),
        )
    };
    let mut block = card_block(
        if title.is_empty() {
            Line::default()
        } else {
            Line::from(Span::styled(
                format!(" {title} "),
                Style::default()
                    .fg(tone.color())
                    .add_modifier(Modifier::BOLD),
            ))
        },
        tone,
    );
    if compact {
        let mut host = host_footer(sample, width.saturating_sub(2));
        if !host.spans.is_empty() {
            host.spans.insert(0, Span::raw(" "));
            host.spans.push(Span::raw(" "));
            block = block.title_bottom(host);
        }
    } else if !footer.is_empty() {
        block = block.title_bottom(plain(format!(" {footer} "), MUTED));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }

    if compact {
        // Reserve state and provenance before fitting even an unusually long
        // model name; a clipped identity must never conceal stale/offline state.
        let status = state(sample);
        let status_width = status.width().min(usize::from(inner.width));
        let identity_width = usize::from(inner.width).saturating_sub(status_width + 2);
        frame.render_widget(
            Paragraph::new(identity(sample, identity_width)),
            Rect::new(inner.x, inner.y, identity_width as u16, 1),
        );
        frame.render_widget(
            Paragraph::new(status),
            Rect::new(
                inner.right() - status_width as u16,
                inner.y,
                status_width as u16,
                1,
            ),
        );
        return;
    }

    let gap = 2;
    let widths = if wide {
        let available = inner.width.saturating_sub(gap * 2);
        let first = available / 3;
        vec![first, first, available - first * 2]
    } else {
        let available = inner.width.saturating_sub(gap);
        let first = available * 55 / 100;
        vec![first, available - first]
    };
    let mut columns = Vec::new();
    let mut x = inner.x;
    for width in widths {
        columns.push(Rect::new(x, inner.y, width, inner.height));
        x += width + gap;
    }
    let runtime_width = usize::from(columns[0].width);
    frame.render_widget(
        Paragraph::new(vec![
            identity(sample, runtime_width),
            state(sample),
            plain(
                fitted_parts(work_parts(sample, history), runtime_width),
                Color::White,
            ),
        ]),
        columns[0],
    );
    let hardware_width = usize::from(columns[1].width);
    let hardware_lines = if wide {
        vec![
            plain(fitted_parts(hardware_parts(sample), hardware_width), BLUE),
            plain(
                fitted_parts(
                    [
                        ram(sample),
                        format!("COMP {}", compressed_memory_label(sample)),
                    ],
                    hardware_width,
                ),
                Color::White,
            ),
            plain(
                gpu_memory(sample).unwrap_or_else(|| "GPU mem —".into()),
                MUTED,
            ),
        ]
    } else {
        vec![
            plain(fitted_parts(hardware_parts(sample), hardware_width), BLUE),
            plain(
                fitted_parts([ram(sample), rss(sample)], hardware_width),
                Color::White,
            ),
            plain(
                fitted_parts(
                    thermal_note(sample).into_iter().chain([cpu(sample)]),
                    hardware_width,
                ),
                MUTED,
            ),
        ]
    };
    frame.render_widget(Paragraph::new(hardware_lines), columns[1]);
    if wide {
        let process_width = usize::from(columns[2].width);
        let footprint = sample
            .process_memory
            .as_ref()
            .map(|memory| {
                format!(
                    "PID {} · OS footprint {}",
                    memory.pid,
                    bytes(memory.footprint)
                )
            })
            .unwrap_or_else(|| "OS footprint —".into());
        frame.render_widget(
            Paragraph::new(vec![
                plain(
                    fitted_parts(
                        [
                            format!("LOCAL {}", process_count_label(sample)),
                            cpu(sample),
                            rss(sample),
                        ],
                        process_width,
                    ),
                    Color::White,
                ),
                plain(compact_label(&footprint, process_width), MUTED),
                plain(fitted_parts(thermal_parts(sample), process_width), MUTED),
            ]),
            columns[2],
        );
    }
}

#[cfg(test)]
#[path = "tests/model_dashboard.rs"]
mod tests;
