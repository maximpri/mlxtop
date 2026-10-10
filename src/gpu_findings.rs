// SPDX-License-Identifier: MIT
//! GPU placement and NVIDIA card problems that limit local inference: a model
//! split onto the CPU, a card whose memory is full, and clocks held down by
//! heat, power or hardware limits. High utilization alone is never a problem.
use crate::domain::Sample;
use crate::formatting::bytes;
use crate::gpu::Throttle;

/// Enter the full state at 97% and leave it below 94%, so a card hovering
/// at its limit does not flap. vLLM and SGLang reserve 90% by default.
pub(crate) const VRAM_FULL_ENTER: u16 = 97;
pub(crate) const VRAM_FULL_EXIT: u16 = 94;
/// Runtimes keep small buffers in RAM even when every layer is on the GPU.
const OFFLOAD_MIN_PERCENT: u64 = 2;
/// Clock limits on an idle or lightly used card cost no throughput.
const THROTTLE_MIN_UTILIZATION: u8 = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
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

    /// Overview guidance: badge, cause and action.
    pub(crate) fn guidance(&self) -> (&'static str, String, &'static str) {
        match self {
            Self::Offload { offloaded, size } => (
                "FIT",
                format!(
                    "{}% of the model runs on CPU — generation is limited by RAM bandwidth.",
                    Self::offload_percent(*offloaded, *size)
                ),
                "Choose a smaller quantization, a shorter context or fewer parallel slots.",
            ),
            Self::VramFull { index, percent, .. } => (
                "VRAM",
                if cfg!(target_os = "windows") {
                    format!("GPU {index} memory {percent}% full — the driver may spill to shared memory.")
                } else {
                    format!("GPU {index} memory {percent}% full — the next allocation may fail.")
                },
                "Unload unused models or reduce context and KV cache.",
            ),
            Self::Throttled { index, cause, .. } => (
                if *cause == Throttle::Thermal {
                    "COOL"
                } else {
                    "POWER"
                },
                format!(
                    "GPU {index} {} under load — clocks are held down.",
                    cause.label()
                ),
                self.next(),
            ),
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
    let has_gpu = sample.has_nvidia_gpus() || cfg!(target_os = "macos");
    if sample.llm_remote || !has_gpu {
        return None;
    }
    let size = sample.llm_model_size.filter(|size| *size > 0)?;
    let offloaded = sample.llm_model_offloaded?.min(size);
    (u128::from(offloaded) * 100 >= u128::from(size) * u128::from(OFFLOAD_MIN_PERCENT))
        .then_some(GpuIssue::Offload { offloaded, size })
}

fn vram_full(sample: &Sample, was_full: bool) -> Option<GpuIssue> {
    if !sample.has_nvidia_gpus() {
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
    if !sample.has_nvidia_gpus() {
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

#[cfg(test)]
#[path = "tests/gpu_findings.rs"]
mod tests;
