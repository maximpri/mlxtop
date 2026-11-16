// SPDX-License-Identifier: MIT
//! GPU placement and discrete card problems that limit local inference: a model
//! split onto the CPU, a card whose memory is full, and clocks held down by
//! heat, power or hardware limits. High utilization alone is never a problem.
//! What else holds a card back (its power cap, a low clock, a slow link, who
//! holds its memory) is evidence in words, never a finding or a colour.
use crate::domain::Sample;
use crate::formatting::bytes;
use crate::gpu::{self, Throttle};

/// Enter the full state at 97% and leave it below 94%, so a card hovering
/// at its limit does not flap. vLLM and SGLang reserve 90% by default.
pub(crate) const VRAM_FULL_ENTER: u16 = 97;
pub(crate) const VRAM_FULL_EXIT: u16 = 94;
/// Runtimes keep small buffers in RAM even when every layer is on the GPU.
const OFFLOAD_MIN_PERCENT: u64 = 2;
/// Clock limits on an idle or lightly used card cost no throughput.
const THROTTLE_MIN_UTILIZATION: u8 = gpu::BUSY_PERCENT;
/// System RAM a card with its own memory holds beyond staging buffers: a
/// model spilling out of VRAM.
const SPILL_BYTES: u64 = 1 << 30;
/// Processes named in a line before the rest are counted.
const PROCESSES_NAMED: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum GpuIssue {
    /// Part of the loaded model runs on the CPU although a GPU is present.
    Offload { offloaded: u64, size: u64 },
    /// One card's memory is at its limit.
    VramFull {
        index: u32,
        used: u64,
        total: u64,
        percent: u16,
    },
    /// A busy card is held below its clocks.
    Throttled {
        index: u32,
        cause: Throttle,
        temperature: Option<u64>,
    },
}

impl GpuIssue {
    pub(crate) fn impact(&self) -> &'static str {
        match self {
            Self::Offload { .. } => "CPU OFFLOAD",
            Self::VramFull { .. } => "VRAM FULL",
            Self::Throttled { .. } => "GPU THROTTLED",
        }
    }

    pub(crate) fn health(&self) -> u16 {
        match self {
            Self::Offload { .. } => 50,
            Self::VramFull { .. } => 55,
            Self::Throttled { .. } => 60,
        }
    }

    pub(crate) fn limiter(&self) -> &'static str {
        match self {
            Self::Offload { .. } => "CPU offload",
            Self::VramFull { .. } => "VRAM full",
            Self::Throttled { .. } => "GPU clock limit",
        }
    }

    fn offload_percent(offloaded: u64, size: u64) -> u64 {
        (u128::from(offloaded) * 100 / u128::from(size.max(1))).min(100) as u64
    }

    pub(crate) fn title(&self) -> String {
        match self {
            Self::Offload { offloaded, size } if offloaded >= size => "Model running on CPU".into(),
            Self::Offload { .. } => "Model partly on CPU".into(),
            Self::VramFull { index, .. } => format!("GPU {index} VRAM full"),
            Self::Throttled { index, cause, .. } => format!("GPU {index} {}", cause.label()),
        }
    }

    pub(crate) fn evidence(&self) -> String {
        match self {
            Self::Offload { offloaded, size } => format!(
                "{}% of model on CPU · {} of {}",
                Self::offload_percent(*offloaded, *size),
                bytes(*offloaded),
                bytes(*size)
            ),
            Self::VramFull {
                used,
                total,
                percent,
                ..
            } => format!("VRAM {percent}% · {} of {}", bytes(*used), bytes(*total)),
            Self::Throttled {
                cause, temperature, ..
            } => match temperature {
                Some(celsius) => format!("Clocks limited by {} · {celsius}°C", cause.label()),
                None => format!("Clocks limited by {}", cause.label()),
            },
        }
    }

    pub(crate) fn next(&self) -> &'static str {
        match self {
            Self::Offload { offloaded, size } if offloaded >= size => {
                "Check the GPU driver; the runtime is not using the GPU."
            }
            Self::Offload { .. } => "Use a smaller quantization or context to fit VRAM.",
            Self::VramFull { .. } if cfg!(target_os = "windows") => {
                "Free VRAM; spill into shared memory is much slower."
            }
            Self::VramFull { .. } => "Free VRAM: unload models or reduce context.",
            Self::Throttled {
                cause: Throttle::PowerBrake,
                ..
            } => "Check the GPU power supply and cables.",
            Self::Throttled {
                cause: Throttle::Thermal,
                ..
            } => "Improve GPU cooling; recheck clocks.",
            Self::Throttled { .. } => "Check GPU cooling and power delivery.",
        }
    }
}

/// The most severe issue: placement first, then memory, then clocks.
pub(crate) fn detect(sample: &Sample, previous_impact: Option<&str>) -> Option<GpuIssue> {
    offload(sample)
        .or_else(|| vram_full(sample, previous_impact == Some("VRAM FULL")))
        .or_else(|| throttled(sample))
}

fn offload(sample: &Sample) -> Option<GpuIssue> {
    // A remote runtime's placement says nothing about local cards, and a
    // CPU-only host has nowhere else to put the model.
    let has_gpu = sample.has_discrete_gpus() || cfg!(target_os = "macos");
    if sample.llm_remote || !has_gpu {
        return None;
    }
    let size = sample.llm_model_size.filter(|size| *size > 0)?;
    let offloaded = sample.llm_model_offloaded?.min(size);
    (u128::from(offloaded) * 100 >= u128::from(size) * u128::from(OFFLOAD_MIN_PERCENT))
        .then_some(GpuIssue::Offload { offloaded, size })
}

fn vram_full(sample: &Sample, was_full: bool) -> Option<GpuIssue> {
    if !sample.has_discrete_gpus() {
        return None;
    }
    let limit = if was_full {
        VRAM_FULL_EXIT
    } else {
        VRAM_FULL_ENTER
    };
    sample
        .gpus
        .iter()
        .filter_map(|gpu| {
            let percent = gpu.memory_percent()?;
            (percent >= limit).then(|| GpuIssue::VramFull {
                index: gpu.index,
                used: gpu.used.unwrap_or_default(),
                total: gpu.total.unwrap_or_default(),
                percent,
            })
        })
        .max_by_key(|issue| match issue {
            GpuIssue::VramFull { percent, .. } => *percent,
            _ => 0,
        })
}

fn throttled(sample: &Sample) -> Option<GpuIssue> {
    if !sample.has_discrete_gpus() {
        return None;
    }
    sample.gpus.iter().find_map(|gpu| {
        let cause = gpu.throttle().filter(|cause| cause.is_fault())?;
        (gpu.utilization? >= THROTTLE_MIN_UTILIZATION).then_some(GpuIssue::Throttled {
            index: gpu.index,
            cause,
            temperature: gpu.temperature,
        })
    })
}

/// What holds each card back, in words for Diagnostics and the static
/// report; none of it is a finding or a colour. A busy card's clock limit
/// with its cause, or a clock under two thirds of its maximum; a busy card's
/// PCIe link below what it and its slot allow; who holds a card's memory,
/// where that was read; and the system RAM a card with VRAM of its own
/// holds beyond staging buffers.
pub(crate) fn evidence(sample: &Sample) -> Vec<String> {
    let mut lines = Vec::new();
    for gpu in &sample.gpus {
        let limits = &gpu.limits;
        let clock = limits
            .clock_mhz
            .map(|_| gpu::clock_label(limits.clock_mhz, limits.clock_max_mhz));
        match gpu.throttle().filter(|_| gpu.busy()) {
            Some(cause) => {
                let mut parts = vec![format!(
                    "GPU {} clocks limited by {}",
                    gpu.index,
                    cause.label()
                )];
                match cause {
                    Throttle::PowerCap if limits.power_mw.is_some() => {
                        parts.push(gpu::power_label(limits.power_mw, limits.power_cap_mw));
                    }
                    Throttle::Thermal => parts.extend(gpu.temperature.map(|c| format!("{c}°C"))),
                    _ => {}
                }
                parts.extend(clock);
                lines.push(parts.join(" · "));
            }
            None => {
                if let (Some(share), Some(clock)) = (gpu.clock_held(), clock) {
                    lines.push(format!(
                        "GPU {} clock at {share}% of its maximum ({clock}) while {}% busy",
                        gpu.index,
                        gpu.utilization.unwrap_or_default()
                    ));
                }
            }
        }
        if gpu.link_reduced() {
            lines.push(format!(
                "GPU {} PCIe link at {} while busy; it and its slot allow {}",
                gpu.index,
                gpu::link_label(limits.link_gen, limits.link_width),
                gpu::link_label(limits.link_gen_max, limits.link_width_max)
            ));
        }
        if !limits.processes.is_empty() {
            let name = |pid: u32| {
                sample
                    .llm_processes
                    .iter()
                    .find(|process| process.pid == pid)
                    .map_or_else(
                        || format!("pid {pid}"),
                        |process| format!("{} ({pid})", process.name),
                    )
            };
            let mut held: Vec<String> = limits
                .processes
                .iter()
                .take(PROCESSES_NAMED)
                .map(|(pid, used)| format!("{} {}", name(*pid), bytes(*used)))
                .collect();
            if limits.processes.len() > PROCESSES_NAMED {
                held.push(format!("{} more", limits.processes.len() - PROCESSES_NAMED));
            }
            lines.push(format!(
                "GPU {} memory by process: {}",
                gpu.index,
                held.join(" · ")
            ));
        }
        if let Some(spill) = limits
            .ram_used
            .filter(|used| !gpu.unified && *used >= SPILL_BYTES)
        {
            lines.push(format!(
                "GPU {} also holds {} of system RAM (GTT)",
                gpu.index,
                bytes(spill)
            ));
        }
    }
    lines
}

/// What to change when a slowdown is put down to heat and a card is holding
/// its clocks down for it: that card's cooling. Short enough to stay whole
/// on the assessment's second line at 80 columns.
pub(crate) fn heat_step(sample: &Sample) -> String {
    let hot = sample
        .gpus
        .iter()
        .find(|gpu| gpu.throttle() == Some(Throttle::Thermal));
    let card = hot.map_or_else(|| "A GPU".to_owned(), |gpu| format!("GPU {}", gpu.index));
    let at = hot
        .and_then(|gpu| gpu.temperature)
        .map(|celsius| format!(" at {celsius}°C"))
        .unwrap_or_default();
    format!("{card} slows its clocks for heat{at}: improve its cooling or airflow.")
}

#[cfg(test)]
#[path = "tests/gpu_findings.rs"]
mod tests;
