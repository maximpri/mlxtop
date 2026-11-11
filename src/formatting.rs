// SPDX-License-Identifier: MIT
//! Human-readable measurements shared by terminal and text reports.
use crate::analysis::llm_context_tokens;
use crate::config::Thresholds;
use crate::domain::{RequestUsage, Sample, TelemetrySource};
use std::time::SystemTime;
pub(crate) fn llm_model_label(sample: &Sample, max_chars: usize) -> String {
    compact_label(&sample.llm_model, max_chars)
}

/// The runtime, with the engine it serves through when that is not its own.
pub(crate) fn llm_runtime_label(sample: &Sample) -> String {
    match &sample.llm_engine {
        Some(engine) => format!("{} · {engine}", sample.llm_provider),
        None => sample.llm_provider.clone(),
    }
}

pub(crate) fn compact_label(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.into();
    }
    let mut label: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    label.push('…');
    label
}

pub(crate) fn tokens_per_second(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.1} tok/s"))
        .unwrap_or_else(|| "—".into())
}

pub(crate) fn llm_rate_label(
    sample: &Sample,
    metric: &str,
    value: Option<f64>,
    live: bool,
) -> String {
    let label = if live {
        metric.to_owned()
    } else if value.is_some() {
        match sample.llm_source {
            TelemetrySource::Live => format!("AVG {metric}"),
            TelemetrySource::Log | TelemetrySource::Report => format!("LAST {metric}"),
            TelemetrySource::None => metric.to_owned(),
        }
    } else {
        metric.to_owned()
    };
    format!("{label} {}", tokens_per_second(value))
}

pub(crate) fn llm_generation_rate_label(sample: &Sample) -> String {
    llm_rate_label(
        sample,
        "GEN",
        sample.llm_generation_tps,
        sample.llm_generation_tps_live,
    )
}

pub(crate) fn llm_prefill_rate_label(sample: &Sample) -> String {
    if sample.llm_provider == "mlx-serve"
        && sample.llm_prefill_tps.is_some()
        && !sample.llm_prefill_tps_live
    {
        // The native runtime keeps the last finished request's prefill speed.
        format!("LAST PREFILL {}", tokens_per_second(sample.llm_prefill_tps))
    } else {
        llm_rate_label(
            sample,
            "PREFILL",
            sample.llm_prefill_tps,
            sample.llm_prefill_tps_live,
        )
    }
}

/// The prefill in flight as tokens forwarded over the tokens it will forward, with its percentage.
pub(crate) fn llm_prefill_progress_label(sample: &Sample) -> Option<String> {
    let expected = sample.llm_prefill_tokens_expected.filter(|n| *n > 0)?;
    let done = sample.llm_prefill_tokens_done.unwrap_or(0).min(expected);
    Some(format!(
        "{} / {} · {}%",
        compact_tokens(done),
        compact_tokens(expected),
        progress_percent(done, expected)
    ))
}

/// Work done as a whole percentage, rounded down: unfinished work never
/// reads 100%, so a prompt with tokens left is not shown as complete.
pub(crate) fn progress_percent(done: u64, total: u64) -> u64 {
    if total == 0 {
        return 0;
    }
    (u128::from(done.min(total)) * 100 / u128::from(total)) as u64
}

/// Time left for work under way. Under a second reads `<1s`, never `0s`,
/// which would say the work has finished.
pub(crate) fn time_left_label(seconds: f64) -> String {
    if seconds.is_finite() && seconds < 1.0 {
        "<1s".into()
    } else {
        duration_label(seconds)
    }
}

pub(crate) fn percent(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.1}%"))
        .unwrap_or_else(|| "—".into())
}

pub(crate) fn percent_u8(value: Option<u8>) -> String {
    value
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "—".into())
}

pub(crate) fn optional_tokens(value: Option<u64>) -> String {
    value.map(compact_tokens).unwrap_or_else(|| "—".into())
}

pub(crate) fn llm_context_label(sample: &Sample) -> String {
    llm_context_tokens(sample)
        .map(compact_tokens)
        .unwrap_or_else(|| "—".into())
}

/// Translate native pressure levels into words that describe the operating
/// condition. Color remains a secondary visual cue; it is never the diagnosis
/// shown to the user.
pub(crate) fn pressure_state_label(sample: &Sample) -> &'static str {
    match sample.pressure.as_str() {
        "GREEN" => "normal",
        "YELLOW" => "watch",
        "RED" => "critical",
        _ => match sample.pressure_meaning.as_str() {
            "normal" => "normal",
            "warning" => "watch",
            "critical" => "critical",
            _ => "unavailable",
        },
    }
}

pub(crate) fn gpu_load_label(value: Option<u8>, thresholds: Thresholds) -> &'static str {
    match value.map(u64::from) {
        Some(value) if value >= thresholds.gpu_critical_load => "saturated",
        Some(value) if value >= thresholds.gpu_warn_load => "busy",
        Some(_) => "within target",
        None => "unavailable",
    }
}

/// Where the runtime reading came from, with its age once it is no longer
/// fresh: a live reading a second old needs no "0s old".
pub(crate) fn telemetry_source(sample: &Sample) -> String {
    let label = match sample.llm_source {
        TelemetrySource::Live => "LIVE",
        TelemetrySource::Log => "LOG",
        TelemetrySource::Report => "REPORTED",
        TelemetrySource::None => return String::new(),
    };
    let fresh = sample.llm_source == TelemetrySource::Live
        && sample.llm_observed_at.is_some_and(|at| {
            SystemTime::now()
                .duration_since(at)
                .is_ok_and(|age| age.as_secs() < 3)
        });
    if fresh {
        label.into()
    } else {
        format!("{label} · {}", telemetry_age(sample.llm_observed_at))
    }
}

/// A short duration for people: "38s", "4m 12s", "1h 5m".
pub(crate) fn duration_label(seconds: f64) -> String {
    let seconds = if seconds.is_finite() {
        seconds.max(0.0).round() as u64
    } else {
        0
    };
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
    }
}

/// A spinner frame that advances with every sample, from the sample's clock.
pub(crate) fn activity_frame(sample: &Sample) -> char {
    const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    sample
        .updated
        .chars()
        .last()
        .and_then(|digit| digit.to_digit(10))
        .map_or('●', |digit| FRAMES[digit as usize])
}

/// Runtime states with work under way, which earn the spinner.
pub(crate) fn llm_is_working(status: &str) -> bool {
    matches!(
        status,
        "generating" | "prefilling" | "loading" | "restarting" | "untracked"
    )
}

pub(crate) fn telemetry_age(observed_at: Option<SystemTime>) -> String {
    let Some(observed_at) = observed_at else {
        return "age —".into();
    };
    let seconds = SystemTime::now()
        .duration_since(observed_at)
        .map(|age| age.as_secs())
        .unwrap_or(0);
    if seconds < 60 {
        format!("{seconds}s old")
    } else if seconds < 3600 {
        format!("{}m old", seconds / 60)
    } else {
        format!("{}h old", seconds / 3600)
    }
}

pub(crate) fn count(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "—".into())
}

pub(crate) fn process_count_label(sample: &Sample) -> String {
    if sample.llm_count == 1 {
        "1 process".into()
    } else {
        format!("{} processes", sample.llm_count)
    }
}

pub(crate) fn compact_tokens(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

pub(crate) fn bytes(value: u64) -> String {
    if value >= 1024_u64.pow(3) {
        format!("{:.1} GiB", value as f64 / 1024_f64.powi(3))
    } else if value >= 1024_u64.pow(2) {
        format!("{:.1} MiB", value as f64 / 1024_f64.powi(2))
    } else if value >= 1024 {
        format!("{:.1} KiB", value as f64 / 1024.0)
    } else {
        format!("{value} B")
    }
}

pub(crate) fn optional_bytes(value: Option<u64>) -> String {
    value.map(bytes).unwrap_or_else(|| "—".into())
}

pub(crate) fn compressed_memory_label(sample: &Sample) -> String {
    if !sample.vm_available {
        return "—".into();
    }
    if sample.compressor == 0 || sample.compressed_logical == 0 {
        return bytes(sample.compressor);
    }
    format!(
        "{} · {:.1}×",
        bytes(sample.compressor),
        sample.compressed_logical as f64 / sample.compressor as f64
    )
}

pub(crate) fn rate(value: u64) -> String {
    format!("{}/s", bytes(value))
}

pub(crate) fn signed_rate(value: i64) -> String {
    if value > 0 {
        format!("+{}", rate(value as u64))
    } else if value < 0 {
        format!("-{}", rate(value.unsigned_abs()))
    } else {
        "0 B/s".into()
    }
}

impl RequestUsage {
    /// One Journal row: the request and what it brings, without the runtime
    /// and model the header already names or readings not yet measured.
    pub fn journal_line(&self) -> String {
        let mut parts = vec![
            self.id.clone(),
            format!("{} prompt", compact_tokens(self.prompt)),
        ];
        if let Some(cached) = self.cached.filter(|_| self.prompt > 0) {
            parts.push(format!(
                "{}% cached",
                progress_percent(cached.min(self.prompt), self.prompt)
            ));
        }
        if self.queued {
            parts.push("queued".into());
        }
        if let Some(out) = self.output.filter(|out| *out > 0) {
            parts.push(format!("{} out", compact_tokens(out)));
        }
        if let Some(speed) = self.output_tps.filter(|_| self.completed) {
            parts.push(format!("{speed:.1} tok/s"));
        }
        if let Some(ttft) = self.ttft_ms {
            parts.push(format!("first token {ttft} ms"));
        }
        parts.join(" · ")
    }

    pub fn summary(&self) -> String {
        let mut text = format!(
            "{} prompt {} · out {} · {} · {} · {}",
            if self.completed {
                "reported"
            } else {
                "observed"
            },
            self.prompt,
            optional_tokens(self.output),
            self.provider,
            self.model,
            self.id
        );
        if let Some(cached) = self.cached {
            text.push_str(&format!(" · cached {cached}"));
        }
        if let Some(speed) = self.output_tps {
            text.push_str(&format!(
                " · output {speed:.1} tok/s ({})",
                if self.completed {
                    "request avg"
                } else {
                    "sampled"
                }
            ));
        }
        if let Some(ttft) = self.ttft_ms {
            text.push_str(&format!(" · first token {ttft} ms (reported)"));
        }
        text
    }
}
