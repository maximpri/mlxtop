// SPDX-License-Identifier: MIT
//! Human-readable one-shot reports using the same observations as the dashboard.
use crate::config::Thresholds;
use crate::domain::Sample;
use crate::formatting::{
    bytes, llm_generation_rate_label, llm_model_label, llm_prefill_progress_label,
    llm_prefill_rate_label, llm_runtime_label, optional_bytes, optional_tokens, percent,
    percent_u8, pressure_state_label, rate, signed_rate, telemetry_source,
};
use crate::host::Platform;
use crate::{diagnosis, gpu_dashboard};
use std::io;
use std::io::Write;
/// Write the `--once` report. `platform` selects the GPU section, so either
/// platform's layout can be checked from any host.
pub(crate) fn write_static(
    out: &mut dyn Write,
    sample: &Sample,
    interval: u64,
    thresholds: Thresholds,
    platform: Platform,
) -> io::Result<()> {
    writeln!(out, "mlxtop · static report ({interval}s sample)\n")?;
    writeln!(out, "SIGNAL       {} · {}", sample.impact, sample.grade)?;
    writeln!(out, "PRESSURE     {}", pressure_state_label(sample))?;
    writeln!(
        out,
        "MEMORY       {} resident / {} total · includes file cache",
        sample
            .resident_memory
            .map(bytes)
            .unwrap_or_else(|| "—".into()),
        bytes(sample.total_memory)
    )?;
    // Windows has no paging counters: unknown, not zero.
    // All disk paging: swap plus file page-ins/outs, then the file share and
    // whether it counts (only while memory is tight).
    let (paging_in, paging_out) = if sample.paging_unavailable {
        ("—".to_string(), "—".to_string())
    } else {
        let file = if sample.page_in.saturating_add(sample.page_out) == 0 {
            String::new()
        } else {
            format!(
                " · file in {} · out {} · memory {}",
                rate(sample.page_in),
                rate(sample.page_out),
                if sample.memory_tight() {
                    "tight"
                } else {
                    "not tight"
                }
            )
        };
        (
            rate(sample.swap_in.saturating_add(sample.page_in)),
            format!(
                "{}{file}",
                rate(sample.swap_out.saturating_add(sample.page_out))
            ),
        )
    };
    if sample.swap_available && sample.swap_total == 0 {
        writeln!(
            out,
            "PAGING       SWAP 0 B · not allocated · in {paging_in} · out {paging_out}"
        )?;
    } else if sample.swap_available {
        let used_percent = sample
            .swap_used
            .saturating_mul(100)
            .checked_div(sample.swap_total)
            .unwrap_or(0);
        writeln!(
            out,
            "PAGING       {} / {} · {}% used · in {paging_in} · out {paging_out}",
            bytes(sample.swap_used),
            bytes(sample.swap_total),
            used_percent,
        )?;
    } else {
        writeln!(out, "PAGING       —")?;
    }
    if sample.has_discrete_gpus() {
        for line in gpu_dashboard::static_lines(&sample.gpus, thresholds) {
            writeln!(out, "{line}")?;
        }
    } else if platform == Platform::MacOs {
        writeln!(
            out,
            "METAL        {} · {} cores · GPU {} · renderer {} · tiler {}",
            sample.metal.device_name.as_deref().unwrap_or("unavailable"),
            sample
                .metal
                .gpu_cores
                .map(|value| value.to_string())
                .unwrap_or_else(|| "—".into()),
            sample
                .gpu_util
                .map(|v| format!("{v}%"))
                .unwrap_or_else(|| "—".into()),
            percent_u8(sample.metal.renderer_util),
            percent_u8(sample.metal.tiler_util)
        )?;
    } else {
        writeln!(out, "GPU          no NVIDIA or AMD counters")?;
    }
    writeln!(
        out,
        "RUNTIME      thermal {} · LLM {} · footprint {} · Metal limit {}",
        sample.thermal,
        sample.llm_count,
        optional_bytes(sample.mlx.process_footprint),
        optional_bytes(sample.metal.resource_limit)
    )?;
    writeln!(
        out,
        "LLM          {} · {} · {} · {}",
        llm_runtime_label(sample),
        sample.llm_status,
        telemetry_source(sample),
        llm_model_label(sample, 40)
    )?;
    if let Some(details) = &sample.llm_details {
        writeln!(out, "PROVIDER     {details}")?;
    }
    writeln!(
        out,
        "SERVING      {} · {}{} · active {} · cache {}",
        llm_generation_rate_label(sample),
        llm_prefill_rate_label(sample),
        llm_prefill_progress_label(sample)
            .map(|progress| format!(" · {progress}"))
            .unwrap_or_default(),
        sample
            .llm_active_requests
            .map(|value| value.to_string())
            .unwrap_or_else(|| "—".into()),
        percent(sample.llm_cache_efficiency)
    )?;
    writeln!(
        out,
        "TOKENS       PROMPT {} · OUT {}",
        optional_tokens(sample.llm_prompt_tokens),
        optional_tokens(sample.llm_output_tokens)
    )?;
    for request in &sample.llm_requests {
        writeln!(out, "REQUEST      {}", request.summary())?;
    }
    if let Some(memory) = &sample.process_memory {
        writeln!(
            out,
            "PROCESS OS   pid {} · footprint {} · lifetime peak {} · RSS {} · growth {}",
            memory.pid,
            bytes(memory.footprint),
            bytes(memory.peak),
            bytes(memory.resident),
            sample
                .process_memory_growth
                .map(signed_rate)
                .unwrap_or_else(|| "—".into())
        )?;
    }
    if let Some(engine) = &sample.llm_engine {
        // The model runs in the engine's process; oMLX's MLX runtime does not hold it.
        writeln!(
            out,
            "ENGINE       {engine} under {} · engine status is private to {}",
            sample.llm_provider, sample.llm_provider
        )?;
    } else {
        writeln!(
            out,
            "MLX          version {} · active {} · cache {} · peak {}",
            sample.mlx.version.as_deref().unwrap_or("—"),
            optional_bytes(sample.mlx.active_memory),
            optional_bytes(sample.mlx.cache_memory),
            optional_bytes(sample.mlx.peak_memory)
        )?;
    }
    if !sample.correlation.summary.is_empty() {
        writeln!(out, "SLOWDOWN      {}", sample.correlation.summary)?;
        if !sample.correlation.details.is_empty() {
            writeln!(out, "UNUSUAL       {}", sample.correlation.details)?;
        }
    } else if !sample.correlation.notes.is_empty() {
        writeln!(
            out,
            "UNUSUAL       {}",
            sample.correlation.notes.join(" · ")
        )?;
    }
    if let Some((usual, slow_below)) = sample.correlation.usual_speed {
        writeln!(
            out,
            "USUAL SPEED   {usual:.1} tok/s · slow below {slow_below:.1}"
        )?;
    }
    let finding = diagnosis::assess(sample);
    writeln!(out, "DIAGNOSIS    {}", finding.title)?;
    writeln!(
        out,
        "EVIDENCE     {} · {}",
        finding.evidence, finding.context
    )?;
    if !finding.next.is_empty() {
        writeln!(
            out,
            "{}         {}",
            if finding.actionable { "CHECK" } else { "NOTE " },
            finding.next
        )?;
    }
    Ok(())
}

/// The expanded panel and doctor report share facts and wording.
pub(crate) fn diagnostic_lines(sample: &Sample) -> Vec<String> {
    let finding = diagnosis::assess(sample);
    let mut lines = vec![
        "ASSESSMENT".into(),
        finding.title,
        finding.evidence,
        finding.context,
    ];
    if !finding.next.is_empty() {
        lines.push(format!(
            "{}: {}",
            if finding.actionable { "Check" } else { "Note" },
            finding.next
        ));
    }
    // What a slowdown is judged against, and everything unusual right now.
    let insight = &sample.correlation;
    if let Some((usual, slow_below)) = insight.usual_speed {
        lines.push(format!(
            "Usual generation speed {usual:.1} tok/s · slow below {slow_below:.1}"
        ));
    }
    lines.extend(insight.notes.iter().map(|note| format!("Unusual: {note}")));
    lines.extend([String::new(), "CONNECTION".into()]);
    lines.extend(sample.runtime.lines(sample));
    lines
}

pub(crate) fn write_doctor(out: &mut dyn Write, sample: &Sample) -> io::Result<bool> {
    let host_ready = sample.total_memory > 0
        && sample.vm_available
        && sample.resident_memory.is_some()
        && sample.pressure != "UNKNOWN";
    writeln!(out, "mlxtop doctor")?;
    writeln!(
        out,
        "Host counters: {}",
        if host_ready {
            "available"
        } else {
            "incomplete"
        }
    )?;
    for line in diagnostic_lines(sample) {
        writeln!(out, "{line}")?;
    }
    Ok(host_ready && !sample.runtime.failed())
}
