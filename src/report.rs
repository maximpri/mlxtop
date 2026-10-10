// SPDX-License-Identifier: MIT
//! Human-readable one-shot reports using the same observations as the dashboard.
use crate::config::Thresholds;
use crate::domain::Sample;
use crate::formatting::{
    bytes, llm_generation_rate_label, llm_model_label, llm_prefill_rate_label, optional_bytes,
    optional_tokens, percent, percent_u8, pressure_state_label, rate, signed_rate,
    telemetry_source,
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
    let (paging_in, paging_out) = if sample.paging_unavailable {
        ("—".to_string(), "—".to_string())
    } else {
        (rate(sample.swap_in), rate(sample.swap_out))
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
    if sample.has_nvidia_gpus() {
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
        writeln!(out, "GPU          NVIDIA counters unavailable")?;
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
        sample.llm_provider,
        sample.llm_status,
        telemetry_source(sample),
        llm_model_label(sample, 40)
    )?;
    if let Some(details) = &sample.llm_details {
        writeln!(out, "PROVIDER     {details}")?;
    }
    writeln!(
        out,
        "SERVING      {} · {} · active {} · cache {}",
        llm_generation_rate_label(sample),
        llm_prefill_rate_label(sample),
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
    writeln!(
        out,
        "MLX          version {} · active {} · cache {} · peak {}",
        sample.mlx.version.as_deref().unwrap_or("—"),
        optional_bytes(sample.mlx.active_memory),
        optional_bytes(sample.mlx.cache_memory),
        optional_bytes(sample.mlx.peak_memory)
    )?;
    if !sample.correlation.summary.is_empty() {
        writeln!(out, "CORRELATION   {}", sample.correlation.summary)?;
        writeln!(
            out,
            "EVIDENCE      {} · {} confidence",
            sample.correlation.details,
            sample.correlation.confidence_label()
        )?;
    }
    let finding = diagnosis::assess(sample);
    writeln!(out, "DIAGNOSIS    {}", finding.title)?;
    writeln!(
        out,
        "EVIDENCE     {} · {}",
        finding.evidence, finding.context
    )?;
    writeln!(
        out,
        "{}         {}",
        if finding.actionable { "CHECK" } else { "NOTE " },
        finding.next
    )?;
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
        format!(
            "{}: {}",
            if finding.actionable { "Check" } else { "Note" },
            finding.next
        ),
        String::new(),
        "CONNECTION".into(),
    ];
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
