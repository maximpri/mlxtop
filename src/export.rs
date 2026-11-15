// SPDX-License-Identifier: MIT
//! The machine-readable snapshot: `mlxtop --once --json` prints one and
//! `mlxtop stream` writes one per sample, one line of JSON each. It is built
//! from a sample, its correlation insight and the verdict the screen shows,
//! never from the sample's own field names, which are the recording format
//! and change with it.
//!
//! What `schema_version` promises:
//! - Units are in the names: `_bytes`, `_bytes_per_s`, `_tok_per_s`,
//!   `_tokens`, `_s`, `_ms` (milliseconds: times since the Unix epoch, and
//!   durations a runtime reports in milliseconds), `_w` and `_celsius`.
//!   Shares are ratios from 0 to 1, named `_ratio`; nothing is a percentage.
//!   `relative_change` is a plain number: -0.5 is half as fast.
//! - A reading that is unknown is `null`, never 0.
//! - Provenance is a `source` tag on the object that holds the figures, and
//!   covers everything inside it that names no other: `host` for what mlxtop
//!   measured from the operating system, `runtime` for what the runtime
//!   reported, `estimate` for mlxtop's own judgement from other readings
//!   (the usual speed and what is slow against it, decode per verify pass,
//!   the context left). A figure whose source varies says so beside it
//!   (`limit_source`).
//! - What only a session knows (the best decode reached here) is in the
//!   snapshots `stream` and `serve` write from their session; `--once`
//!   watches none, so it is `null` there.
//! - No process names, command lines or process ids: the stream is meant to
//!   cross a network. Runtime and model names stay.
//! - Fields are added without a new version, so a reader ignores fields it
//!   does not know. Renaming or removing one, or changing its unit or
//!   meaning, takes a new `schema_version`. Experimental through 3.4.
//!
//! To add a field, declare it in its object with its unit in its name and
//! as an `Option` wherever the reading can be unknown, fill it in that
//! object's `of`, and add it to `tests/fixtures/export_v1.json` and to the
//! field list in docs/USER_GUIDE.md: the tests hold the output to both.
use crate::collector::{epoch_millis, Collector};
use crate::diagnosis::{assess, assess_session, Verdict};
use crate::domain::{CorrelationCause, DropEnd, Phase, Sample, TelemetrySource, Tone};
use crate::efficiency::Record;
use crate::host::Platform;
use serde::Serialize;
use std::io::{self, Write};
use std::time::{Duration, Instant};

/// The layout this build writes. Experimental through 3.4: until then a
/// field may still change without a new number.
pub(crate) const SCHEMA_VERSION: u32 = 1;

/// How often a stream that is waiting for its next sample looks for a
/// signal to stop.
const STOP_CHECK: Duration = Duration::from_millis(100);

/// Where an object's figures come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Source {
    /// Measured by mlxtop from the operating system.
    Host,
    /// Reported by the runtime: its API, its completion log or a usage file.
    Runtime,
    /// mlxtop's own judgement from other readings, never a reading itself.
    Estimate,
}

/// One sample, as `--once --json` prints it and `stream` writes it.
#[derive(Serialize)]
pub(crate) struct Snapshot {
    pub(crate) schema_version: u32,
    pub(crate) mlxtop_version: String,
    /// When the sample was taken, in milliseconds since the Unix epoch.
    pub(crate) ts_ms: Option<u64>,
    /// Seconds between samples: the span rates are measured over.
    pub(crate) interval_s: u64,
    pub(crate) verdict: VerdictLine,
    pub(crate) host: Host,
    pub(crate) runtime: Runtime,
    /// Decode per verify pass against the chip's peak, while one request
    /// decodes alone and every input is known.
    pub(crate) efficiency: Option<Efficiency>,
    /// How much more context fits, while every input is known.
    pub(crate) context_left: Option<ContextLeft>,
    /// The usual speed for the work being done, once there is one.
    pub(crate) usual_speed: Option<UsualSpeed>,
    /// What else is unusual in this sample, the most telling first: an
    /// estimate, against the same signals' usual values.
    pub(crate) unusual: Vec<Unusual>,
    /// The slowdown or stall under way, if any.
    pub(crate) slowdown: Option<Slowdown>,
    /// Set on the one sample where a slowdown ended.
    pub(crate) slowdown_ended: Option<SlowdownEnd>,
}

/// The snapshot of `sample`, taken `interval_s` seconds after the one
/// before it on a `platform` machine, with no session behind it: what
/// `--once --json` prints.
pub(crate) fn snapshot(sample: &Sample, platform: Platform, interval_s: u64) -> Snapshot {
    build(sample, platform, interval_s, None)
}

/// The snapshot of `sample`, the sample `collector` just took, as its
/// session sees it, the way the dashboard does: with what the session
/// watched decode reach, so the best share and a healthy verdict's note
/// near it are known. What `mlxtop stream` writes and `mlxtop serve`
/// answers with.
pub(crate) fn session_snapshot(
    collector: &Collector,
    sample: &Sample,
    interval_s: u64,
) -> Snapshot {
    build(
        sample,
        collector.platform,
        interval_s,
        Some(&collector.session.decode),
    )
}

fn build(
    sample: &Sample,
    platform: Platform,
    interval_s: u64,
    decode: Option<&Record>,
) -> Snapshot {
    let insight = &sample.correlation;
    Snapshot {
        schema_version: SCHEMA_VERSION,
        mlxtop_version: crate::cli::VERSION.into(),
        ts_ms: sample.ts,
        interval_s,
        verdict: VerdictLine::of(sample, decode),
        host: Host::of(sample, platform),
        runtime: Runtime::of(sample),
        efficiency: Efficiency::of(sample, decode),
        context_left: ContextLeft::of(sample, platform),
        usual_speed: UsualSpeed::of(sample),
        unusual: insight
            .unusual
            .iter()
            .map(|cause| Unusual {
                cause: cause_name(*cause),
                note: insight.note(*cause).map(str::to_owned),
            })
            .collect(),
        slowdown: Slowdown::of(sample),
        slowdown_ended: insight.ended.map(|end| SlowdownEnd::of(end, insight.phase)),
    }
}

/// The verdict line: its name, then its words as the screen shows them.
#[derive(Serialize)]
pub(crate) struct VerdictLine {
    pub(crate) state: Verdict,
    /// The headline's icon: `ok` ✓, `warning` ▲, `critical` ✖ or `neutral` ●.
    pub(crate) severity: &'static str,
    pub(crate) title: String,
    pub(crate) evidence: String,
    pub(crate) context: String,
    /// What to do or know next, as the line under the headline says it.
    pub(crate) next: Option<String>,
    /// `next` is a step to take (→ on screen), not a note.
    pub(crate) actionable: bool,
}

impl VerdictLine {
    /// As the dashboard words it with a session (`decode`), else as the
    /// static report does.
    fn of(sample: &Sample, decode: Option<&Record>) -> Self {
        let finding = match decode {
            Some(decode) => assess_session(sample, decode),
            None => assess(sample),
        };
        Self {
            state: finding.verdict,
            severity: match finding.tone {
                Tone::Green => "ok",
                Tone::Yellow => "warning",
                Tone::Red => "critical",
                _ => "neutral",
            },
            title: finding.title,
            evidence: finding.evidence,
            context: finding.context,
            next: Some(finding.next).filter(|next| !next.is_empty()),
            actionable: finding.actionable,
        }
    }
}

/// The machine being watched, as mlxtop measured it.
#[derive(Serialize)]
pub(crate) struct Host {
    pub(crate) source: Source,
    /// `macos`, `linux` or `windows`.
    pub(crate) platform: &'static str,
    /// The CPU architecture: `arm64`, `x86_64`.
    pub(crate) arch: Option<String>,
    /// The Apple chip, as its GPU is named (`Apple M4 Max`).
    pub(crate) chip: Option<String>,
    /// The Apple GPU's cores.
    pub(crate) gpu_cores: Option<u16>,
    pub(crate) ram_bytes: Option<u64>,
    /// The peak memory bandwidth the chip's maker publishes: the Mac's
    /// chip, or the one NVIDIA or AMD card. Unknown where mlxtop's table
    /// does not hold the chip or cannot tell its bin, and with several cards.
    pub(crate) published_bandwidth_bytes_per_s: Option<u64>,
    /// `estimate`: which chip this is, and so its figure, is mlxtop's
    /// judgement from its name, GPU cores and memory, not a reading.
    pub(crate) published_bandwidth_source: Option<Source>,
    pub(crate) memory: Memory,
    /// The Apple GPU of a Mac, or each NVIDIA and AMD card.
    pub(crate) gpus: Vec<Gpu>,
    pub(crate) thermal: Thermal,
    /// What the whole machine draws, where macOS reports it.
    pub(crate) system_power_w: Option<f64>,
    pub(crate) runtime_process: RuntimeProcess,
}

impl Host {
    fn of(sample: &Sample, platform: Platform) -> Self {
        let apple = platform == Platform::MacOs;
        let bandwidth = published_bandwidth(sample, platform);
        Self {
            source: Source::Host,
            platform: match platform {
                Platform::MacOs => "macos",
                Platform::Linux => "linux",
                Platform::Windows => "windows",
            },
            arch: sample.metal.architecture.clone(),
            chip: sample.metal.device_name.clone().filter(|_| apple),
            gpu_cores: sample.metal.gpu_cores.filter(|_| apple),
            ram_bytes: known(sample.total_memory),
            published_bandwidth_bytes_per_s: bandwidth,
            published_bandwidth_source: bandwidth.map(|_| Source::Estimate),
            memory: Memory::of(sample, platform),
            gpus: Gpu::all(sample, platform),
            thermal: Thermal::of(sample),
            system_power_w: sample
                .system_power_mw
                .map(|milliwatts| round(milliwatts as f64 / 1000.0, 3)),
            runtime_process: RuntimeProcess::of(sample),
        }
    }
}

/// The peak memory bandwidth the maker publishes for the chip that serves,
/// in bytes a second: the Mac's chip, or the one card, as the host footer
/// names it, decided by the platform the sample was taken on.
fn published_bandwidth(sample: &Sample, platform: Platform) -> Option<u64> {
    let bandwidth = match platform {
        Platform::MacOs => crate::chip_facts::lookup(
            sample.metal.device_name.as_deref()?,
            sample.metal.gpu_cores,
            Some(sample.total_memory),
        ),
        // Several cards have no single figure.
        _ => match sample.gpus.as_slice() {
            [card] => crate::chip_facts::card(card),
            _ => None,
        },
    };
    bandwidth.map(|bandwidth| (bandwidth.gb_s * 1e9).round() as u64)
}

/// Memory laid out as the hardware is: one pool where the GPU shares RAM,
/// VRAM beside RAM where the cards have memory of their own.
#[derive(Serialize)]
pub(crate) struct Memory {
    /// `unified` (the GPU's memory is RAM) or `dedicated` (cards with VRAM).
    pub(crate) layout: Option<&'static str>,
    /// The operating system's memory pressure: `normal`, `warning` or `critical`.
    pub(crate) pressure: Option<&'static str>,
    /// Pressure is raised or little memory is free, so file paging counts
    /// against the machine.
    pub(crate) tight: Option<bool>,
    /// The share of RAM the operating system reports available.
    pub(crate) available_ratio: Option<f64>,
    pub(crate) ram: Ram,
    /// The GPU's part of RAM, on unified memory.
    pub(crate) gpu: Option<GpuShare>,
    /// The cards' own memory, all cards together.
    pub(crate) vram: Option<Vram>,
    pub(crate) swap: Swap,
    pub(crate) file_paging: FilePaging,
    pub(crate) compression: Compression,
}

impl Memory {
    fn of(sample: &Sample, platform: Platform) -> Self {
        let layout = match gpu_memory(sample, platform) {
            crate::gpu::Memory::Shared => Some("unified"),
            crate::gpu::Memory::Dedicated { .. } => Some("dedicated"),
            crate::gpu::Memory::Unknown => None,
        };
        let pressure = match crate::formatting::pressure_state_label(sample) {
            "normal" => Some("normal"),
            "watch" => Some("warning"),
            "critical" => Some("critical"),
            _ => None,
        };
        Self {
            layout,
            pressure,
            tight: (pressure.is_some() || sample.availability.is_some())
                .then(|| sample.memory_tight()),
            available_ratio: sample.availability.map(percent_ratio),
            ram: Ram::of(sample, platform),
            gpu: GpuShare::of(sample, platform),
            vram: Vram::of(sample, platform),
            swap: Swap::of(sample),
            file_paging: FilePaging::of(sample, platform),
            compression: Compression::of(sample, platform),
        }
    }
}

/// All of RAM by what holds it, as the memory panel's bar; all of it is
/// `host.ram_bytes`.
#[derive(Serialize)]
pub(crate) struct Ram {
    /// Wired, app and compressed memory: what the RAM meter fills.
    pub(crate) used_bytes: Option<u64>,
    /// Free memory and file cache: what a model can still grow into.
    pub(crate) left_bytes: Option<u64>,
    /// Memory that cannot be paged out; on Linux, the kernel's and other
    /// memory that cannot be reclaimed.
    pub(crate) wired_bytes: Option<u64>,
    pub(crate) app_bytes: Option<u64>,
    /// What the compressor holds (macOS).
    pub(crate) compressed_bytes: Option<u64>,
    pub(crate) file_cache_bytes: Option<u64>,
    pub(crate) free_bytes: Option<u64>,
}

impl Ram {
    fn of(sample: &Sample, platform: Platform) -> Self {
        let sizes = crate::memory_composition::segments(sample);
        // Windows reports only what is in use and what is available.
        let kinds = sizes.filter(|_| platform != Platform::Windows);
        let kind = |index: usize| kinds.map(|sizes| sizes[index]);
        Self {
            used_bytes: sizes.map(|[wired, app, compressed, ..]| wired + app + compressed),
            left_bytes: sizes.map(crate::memory_headroom::left),
            wired_bytes: kind(0),
            app_bytes: kind(1),
            compressed_bytes: kind(2).filter(|_| platform == Platform::MacOs),
            file_cache_bytes: kind(3),
            free_bytes: kind(4),
        }
    }
}

/// What the GPU holds of RAM where it has no memory of its own, against
/// the limit on what it may wire.
#[derive(Serialize)]
pub(crate) struct GpuShare {
    pub(crate) used_bytes: Option<u64>,
    /// The GPU's wired-memory limit: `iogpu.wired_limit_mb`, else Metal's
    /// recommended working set as the runtime's MLX reports it.
    pub(crate) limit_bytes: Option<u64>,
    /// `host` for the macOS setting, `runtime` for the runtime's report.
    pub(crate) limit_source: Option<Source>,
    /// The room left under the limit; 0 once over it.
    pub(crate) left_bytes: Option<u64>,
}

impl GpuShare {
    fn of(sample: &Sample, platform: Platform) -> Option<Self> {
        if gpu_memory(sample, platform) != crate::gpu::Memory::Shared {
            return None;
        }
        let limit = crate::memory_composition::gpu_limit(sample);
        // The collector takes the runtime's Metal limit only where macOS
        // sets none of its own.
        let reported = sample.metal.resource_limit.is_none()
            || sample.metal.resource_limit == sample.mlx.resource_limit;
        // Held against the limit as the memory panel holds it: Metal's own
        // count, else wired RAM, which bounds it.
        let held = sample
            .gpu_in_use
            .or(sample.vm_available.then_some(sample.wired));
        Some(Self {
            used_bytes: sample.gpu_in_use,
            limit_bytes: limit,
            limit_source: limit.map(|_| {
                if reported {
                    Source::Runtime
                } else {
                    Source::Host
                }
            }),
            left_bytes: limit
                .zip(held)
                .map(|(limit, held)| limit.saturating_sub(held)),
        })
    }
}

/// The cards' own memory, all cards together.
#[derive(Serialize)]
pub(crate) struct Vram {
    pub(crate) used_bytes: u64,
    pub(crate) total_bytes: u64,
    pub(crate) left_bytes: u64,
}

impl Vram {
    fn of(sample: &Sample, platform: Platform) -> Option<Self> {
        match gpu_memory(sample, platform) {
            crate::gpu::Memory::Dedicated { used, total } => Some(Self {
                used_bytes: used,
                total_bytes: total,
                left_bytes: total.saturating_sub(used),
            }),
            _ => None,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct Swap {
    pub(crate) total_bytes: Option<u64>,
    pub(crate) used_bytes: Option<u64>,
    pub(crate) in_bytes_per_s: Option<u64>,
    pub(crate) out_bytes_per_s: Option<u64>,
    /// How fast swap use is growing (negative while it shrinks).
    pub(crate) growth_bytes_per_s: Option<i64>,
}

impl Swap {
    fn of(sample: &Sample) -> Self {
        let size = sample.swap_available;
        let rates = sample.rate_ready && !sample.paging_unavailable;
        Self {
            total_bytes: size.then_some(sample.swap_total),
            used_bytes: size.then_some(sample.swap_used),
            in_bytes_per_s: rates.then_some(sample.swap_in),
            out_bytes_per_s: rates.then_some(sample.swap_out),
            growth_bytes_per_s: (size && sample.rate_ready).then_some(sample.swap_growth),
        }
    }
}

/// Pages read from and written to disk outside swap (macOS): model loads
/// read through here, and a full Mac with swap off pages here.
#[derive(Serialize)]
pub(crate) struct FilePaging {
    pub(crate) in_bytes_per_s: Option<u64>,
    pub(crate) out_bytes_per_s: Option<u64>,
}

impl FilePaging {
    fn of(sample: &Sample, platform: Platform) -> Self {
        let measured =
            platform == Platform::MacOs && sample.rate_ready && !sample.paging_unavailable;
        Self {
            in_bytes_per_s: measured.then_some(sample.page_in),
            out_bytes_per_s: measured.then_some(sample.page_out),
        }
    }
}

/// The memory compressor's work (macOS).
#[derive(Serialize)]
pub(crate) struct Compression {
    pub(crate) compress_bytes_per_s: Option<u64>,
    pub(crate) decompress_bytes_per_s: Option<u64>,
}

impl Compression {
    fn of(sample: &Sample, platform: Platform) -> Self {
        let measured = platform == Platform::MacOs && sample.rate_ready && sample.vm_available;
        Self {
            compress_bytes_per_s: measured.then_some(sample.compress),
            decompress_bytes_per_s: measured.then_some(sample.decompress),
        }
    }
}

/// One GPU: the Apple GPU of a Mac, or an NVIDIA or AMD card.
#[derive(Serialize)]
pub(crate) struct Gpu {
    pub(crate) index: u32,
    /// `apple`, `nvidia` or `amd`.
    pub(crate) vendor: &'static str,
    pub(crate) name: Option<String>,
    pub(crate) busy_ratio: Option<f64>,
    /// The Apple GPU's renderer and tiler, each busy for this share.
    pub(crate) renderer_ratio: Option<f64>,
    pub(crate) tiler_ratio: Option<f64>,
    /// A card's own memory.
    pub(crate) memory_used_bytes: Option<u64>,
    pub(crate) memory_total_bytes: Option<u64>,
    pub(crate) temperature_celsius: Option<u64>,
    /// Why the driver holds clocks down: `thermal`, `power_brake`,
    /// `hardware_slowdown`, `power_cap`, or `none`.
    pub(crate) throttle: Option<&'static str>,
}

impl Gpu {
    fn all(sample: &Sample, platform: Platform) -> Vec<Self> {
        if platform != Platform::MacOs {
            return sample.gpus.iter().map(Self::card).collect();
        }
        vec![Self {
            index: 0,
            vendor: "apple",
            name: sample.metal.device_name.clone(),
            busy_ratio: sample.gpu_util.map(percent_ratio),
            renderer_ratio: sample.metal.renderer_util.map(percent_ratio),
            tiler_ratio: sample.metal.tiler_util.map(percent_ratio),
            memory_used_bytes: None,
            memory_total_bytes: None,
            temperature_celsius: None,
            throttle: None,
        }]
    }

    fn card(card: &crate::gpu::Device) -> Self {
        use crate::gpu::Throttle;
        Self {
            index: card.index,
            vendor: match card.vendor {
                crate::gpu::Vendor::Nvidia => "nvidia",
                crate::gpu::Vendor::Amd => "amd",
            },
            name: Some(card.name.clone()),
            busy_ratio: card.utilization.map(percent_ratio),
            renderer_ratio: None,
            tiler_ratio: None,
            memory_used_bytes: card.used,
            memory_total_bytes: card.total,
            temperature_celsius: card.temperature,
            throttle: card.throttle_reasons.map(|_| match card.throttle() {
                Some(Throttle::Thermal) => "thermal",
                Some(Throttle::PowerBrake) => "power_brake",
                Some(Throttle::HardwareSlowdown) => "hardware_slowdown",
                Some(Throttle::PowerCap) => "power_cap",
                None => "none",
            }),
        }
    }
}

/// Heat, as the operating system reports it.
#[derive(Serialize)]
pub(crate) struct Thermal {
    /// macOS's thermal pressure level: `nominal`, `moderate`, `heavy`,
    /// `trapping` or `sleeping`.
    pub(crate) level: Option<&'static str>,
    /// macOS is holding the machine back for heat: pressure from `heavy`
    /// up, or a CPU speed limit.
    pub(crate) limited: Option<bool>,
    /// The share of full CPU speed macOS allows (Intel Macs).
    pub(crate) cpu_speed_limit_ratio: Option<f64>,
    /// The hottest temperature measured: thermal zones and GPUs on Linux,
    /// GPUs on Windows.
    pub(crate) hottest_celsius: Option<u64>,
}

impl Thermal {
    fn of(sample: &Sample) -> Self {
        let state = &sample.thermal_state;
        Self {
            level: state.level.map(|level| level.label()),
            limited: state.rank().map(|_| state.limited()),
            cpu_speed_limit_ratio: state.cpu_speed_limit.map(percent_ratio),
            // Linux and Windows record the hottest reading in words.
            hottest_celsius: sample
                .thermal
                .strip_suffix("°C measured")
                .and_then(|degrees| degrees.parse().ok()),
        }
    }
}

/// The runtime's processes as the operating system sees them, by count:
/// no names.
#[derive(Serialize)]
pub(crate) struct RuntimeProcess {
    pub(crate) count: u32,
    /// What the runtime holds: its largest process's footprint (GPU memory
    /// included) or the resident size of all its processes, whichever is more.
    pub(crate) memory_bytes: Option<u64>,
    /// Its largest process's lifetime peak footprint, and how fast its
    /// footprint is growing.
    pub(crate) peak_bytes: Option<u64>,
    pub(crate) growth_bytes_per_s: Option<i64>,
    /// CPU its processes use, in cores.
    pub(crate) cpu_cores: Option<f64>,
    /// Its storage reads and writes: a prompt cache on disk, or weights
    /// streamed from it.
    pub(crate) disk_read_bytes_per_s: Option<u64>,
    pub(crate) disk_write_bytes_per_s: Option<u64>,
}

impl RuntimeProcess {
    fn of(sample: &Sample) -> Self {
        let running = sample.llm_count > 0;
        Self {
            count: sample.llm_count,
            memory_bytes: Some(sample.runtime_memory()).filter(|bytes| running && *bytes > 0),
            peak_bytes: sample.process_memory.as_ref().map(|reading| reading.peak),
            growth_bytes_per_s: sample.process_memory_growth,
            cpu_cores: running.then(|| round(sample.llm_cpu / 100.0, 2)),
            disk_read_bytes_per_s: sample.disk.read,
            disk_write_bytes_per_s: sample.disk.write,
        }
    }
}

/// The runtime serving the model, and what it reports.
#[derive(Serialize)]
pub(crate) struct Runtime {
    pub(crate) source: Source,
    pub(crate) provider: Option<String>,
    /// The engine it serves through when that is not its own (oMLX's Splash).
    pub(crate) engine: Option<String>,
    pub(crate) model: Option<String>,
    /// Its state in its own word: `generating`, `prefilling`, `idle` …
    pub(crate) status: Option<String>,
    /// Where its readings came from: `live_api`, `completion_log` (the last
    /// finished request in its log) or `reported_usage` (a usage file).
    pub(crate) telemetry: Option<&'static str>,
    /// Its readings are live and current.
    pub(crate) fresh: bool,
    /// When it last answered, in milliseconds since the Unix epoch.
    pub(crate) observed_at_ms: Option<u64>,
    /// Its API answered the last poll.
    pub(crate) connected: Option<bool>,
    /// It runs on another machine: the host readings here are not its own.
    pub(crate) remote: bool,
    /// Its own facts in words, as the static report's PROVIDER line.
    pub(crate) details: Option<String>,
    pub(crate) model_memory_bytes: Option<u64>,
    /// The most memory it lets its models take.
    pub(crate) model_memory_ceiling_bytes: Option<u64>,
    pub(crate) model_size_bytes: Option<u64>,
    /// The part of the model it placed outside GPU memory.
    pub(crate) model_offloaded_bytes: Option<u64>,
    /// The context it is set to allow for the model.
    pub(crate) context_allowed_tokens: Option<u64>,
    /// KV-cache room it reports left, in tokens: a pool its requests share,
    /// or for llama.cpp the room of the running slot nearest its limit.
    pub(crate) kv_free_tokens: Option<u64>,
    /// KV-cache room it reports left, in bytes: what its own memory plan has
    /// left (Splash), or its free tokens at the bytes a token costs (SGLang).
    pub(crate) kv_free_bytes: Option<u64>,
    /// Requests it preempted, retracted or suspended for want of KV cache,
    /// by its own count since it started.
    pub(crate) preemptions_total: Option<u64>,
    /// What the model costs a pass, a token and a sequence, as mlxtop
    /// works it out from the model's own files; `null` when nothing is known.
    pub(crate) model_facts: Option<ModelFacts>,
    /// The MLX allocator inside it, where it reports one.
    pub(crate) mlx: Option<Mlx>,
    pub(crate) generation: Generation,
    pub(crate) prefill: Prefill,
    pub(crate) requests: Requests,
    pub(crate) cache: Cache,
    pub(crate) draft: Draft,
}

impl Runtime {
    fn of(sample: &Sample) -> Self {
        let provider = Some(sample.llm_provider.clone())
            .filter(|provider| !matches!(provider.as_str(), "none" | "not detected" | ""));
        let found = provider.is_some();
        Self {
            source: Source::Runtime,
            provider,
            engine: sample.llm_engine.clone(),
            // With no runtime answering, the model is named after its
            // process: that name stays on this machine.
            model: Some(sample.llm_model.clone()).filter(|model| {
                !model.is_empty() && model != "not detected" && *model != sample.llm_top
            }),
            status: Some(sample.llm_status.clone()).filter(|status| found && !status.is_empty()),
            telemetry: match sample.llm_source {
                TelemetrySource::Live => Some("live_api"),
                TelemetrySource::Log => Some("completion_log"),
                TelemetrySource::Report => Some("reported_usage"),
                TelemetrySource::None => None,
            },
            fresh: sample.llm_fresh(),
            observed_at_ms: sample.llm_observed_at.and_then(epoch_millis),
            // Discovery that found nothing is not a runtime that is down.
            connected: sample
                .runtime
                .attempted_at
                .filter(|_| sample.runtime.provider.is_some())
                .map(|_| sample.runtime.connected),
            remote: sample.llm_remote,
            details: sample.llm_details.clone(),
            model_memory_bytes: sample.llm_model_memory,
            model_memory_ceiling_bytes: sample.llm_model_memory_max,
            model_size_bytes: sample.llm_model_size,
            model_offloaded_bytes: sample.llm_model_offloaded,
            context_allowed_tokens: sample.model.context_configured,
            kv_free_tokens: sample.llm_kv_free_tokens,
            kv_free_bytes: sample.llm_kv_free_bytes,
            preemptions_total: sample.llm_preemptions_total,
            model_facts: ModelFacts::of(&sample.model),
            mlx: Mlx::of(sample),
            generation: Generation::of(sample),
            prefill: Prefill::of(sample),
            requests: Requests::of(sample),
            cache: Cache::of(sample),
            draft: Draft::of(sample),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct Mlx {
    pub(crate) version: Option<String>,
    pub(crate) active_bytes: Option<u64>,
    pub(crate) cache_bytes: Option<u64>,
    pub(crate) peak_bytes: Option<u64>,
    /// The serving process's footprint as the runtime reports it.
    pub(crate) process_footprint_bytes: Option<u64>,
}

impl Mlx {
    fn of(sample: &Sample) -> Option<Self> {
        let mlx = &sample.mlx;
        let mlx = Self {
            version: mlx.version.clone(),
            active_bytes: mlx.active_memory,
            cache_bytes: mlx.cache_memory,
            peak_bytes: mlx.peak_memory,
            process_footprint_bytes: mlx.process_footprint,
        };
        let reported = mlx.version.is_some()
            || [
                mlx.active_bytes,
                mlx.cache_bytes,
                mlx.peak_bytes,
                mlx.process_footprint_bytes,
            ]
            .iter()
            .any(Option::is_some);
        reported.then_some(mlx)
    }
}

/// What the serving model costs, as mlxtop works it out from the model's
/// `config.json` and safetensors headers and the runtime's settings: an
/// estimate, except a figure that names the runtime as its source.
#[derive(Serialize)]
pub(crate) struct ModelFacts {
    pub(crate) source: Source,
    /// Weight bytes one forward pass reads for a token: the decoder layers
    /// and the output head; for MoE, the experts one token uses.
    pub(crate) weight_bytes_per_pass: Option<u64>,
    /// Bits per parameter a pass reads, scales and biases included.
    pub(crate) bits_per_weight: Option<f64>,
    /// KV-cache bytes one more token of context costs.
    pub(crate) kv_bytes_per_token: Option<u64>,
    /// `runtime` where the runtime reports it, `estimate` where mlxtop
    /// works it out from the model's layers.
    pub(crate) kv_bytes_per_token_source: Option<Source>,
    /// What a sequence holds whatever its length: linear-attention state,
    /// full sliding windows, a draft's context window. 0 for a model with none.
    pub(crate) state_bytes_per_sequence: Option<u64>,
    /// The context the model was trained for, as its `config.json` (else
    /// the runtime) declares it.
    pub(crate) context_trained_tokens: Option<u64>,
    /// The draft's weight bytes as stored, where speculative decoding drafts.
    pub(crate) draft_weight_bytes: Option<u64>,
}

impl ModelFacts {
    fn of(facts: &crate::model_facts::ModelFacts) -> Option<Self> {
        let kv_bytes_per_token = facts.kv_bytes_per_token();
        let model = Self {
            source: Source::Estimate,
            weight_bytes_per_pass: facts.bytes_per_pass,
            bits_per_weight: facts
                .bits_per_weight
                .filter(|bits| bits.is_finite() && *bits > 0.0)
                .map(|bits| round(bits, 3)),
            kv_bytes_per_token,
            kv_bytes_per_token_source: kv_bytes_per_token.map(|_| {
                if facts.kv_bytes_per_token_reported.is_some() {
                    Source::Runtime
                } else {
                    Source::Estimate
                }
            }),
            state_bytes_per_sequence: facts.fixed_state_bytes,
            context_trained_tokens: facts.context_trained,
            draft_weight_bytes: facts.draft_bytes,
        };
        let known = model.weight_bytes_per_pass.is_some()
            || model.bits_per_weight.is_some()
            || model.kv_bytes_per_token.is_some()
            || model.state_bytes_per_sequence.is_some()
            || model.context_trained_tokens.is_some()
            || model.draft_weight_bytes.is_some();
        known.then_some(model)
    }
}

/// What a rate is: measured over the last interval, the runtime's average
/// while no live rate is reported, or the last finished request's, as the
/// screen's GEN, AVG GEN and LAST GEN.
fn basis(sample: &Sample, rate: Option<f64>, live: bool) -> Option<&'static str> {
    rate?;
    if live {
        return Some("live");
    }
    match sample.llm_source {
        TelemetrySource::Live => Some("average"),
        TelemetrySource::Log | TelemetrySource::Report => Some("last_request"),
        TelemetrySource::None => None,
    }
}

/// How fast the answer is written.
#[derive(Serialize)]
pub(crate) struct Generation {
    pub(crate) tok_per_s: Option<f64>,
    /// `live`, `average` or `last_request`.
    pub(crate) basis: Option<&'static str>,
}

impl Generation {
    fn of(sample: &Sample) -> Self {
        let rate = sample.llm_generation_tps.and_then(tok_per_s);
        Self {
            tok_per_s: rate,
            basis: basis(sample, rate, sample.llm_generation_tps_live),
        }
    }
}

/// How fast prompts are read, and the prompt being read.
#[derive(Serialize)]
pub(crate) struct Prefill {
    pub(crate) tok_per_s: Option<f64>,
    /// `live`, `average` or `last_request`.
    pub(crate) basis: Option<&'static str>,
    pub(crate) done_tokens: Option<u64>,
    pub(crate) expected_tokens: Option<u64>,
}

impl Prefill {
    fn of(sample: &Sample) -> Self {
        let rate = sample.llm_prefill_tps.and_then(tok_per_s);
        // mlx-serve keeps the last finished request's prefill speed.
        let kept = sample.llm_provider == "mlx-serve" && !sample.llm_prefill_tps_live;
        Self {
            tok_per_s: rate,
            basis: if kept && rate.is_some() {
                Some("last_request")
            } else {
                basis(sample, rate, sample.llm_prefill_tps_live)
            },
            done_tokens: sample.llm_prefill_tokens_done,
            expected_tokens: sample.llm_prefill_tokens_expected,
        }
    }
}

/// The requests the runtime is serving.
#[derive(Serialize)]
pub(crate) struct Requests {
    pub(crate) active: Option<u64>,
    pub(crate) waiting: Option<u64>,
    /// Running requests reading their prompt and writing their answer,
    /// where the runtime lists its requests.
    pub(crate) prefilling: Option<u64>,
    pub(crate) generating: Option<u64>,
    /// Tokens the requests writing an answer hold, prompt and output
    /// together: the context their speed is judged at.
    pub(crate) context_tokens: Option<u64>,
    /// The runtime's prompt and output token counts, as the static report's
    /// TOKENS line.
    pub(crate) prompt_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) list: Vec<Request>,
}

impl Requests {
    fn of(sample: &Sample) -> Self {
        let work = crate::analysis::CorrelationObservation::from_sample(sample);
        let listed = !sample.llm_requests.is_empty();
        Self {
            active: sample.llm_active_requests,
            waiting: sample.llm_waiting_requests,
            prefilling: listed.then_some(work.prefilling),
            generating: listed.then_some(work.generating),
            context_tokens: work.context,
            prompt_tokens: sample.llm_prompt_tokens,
            output_tokens: sample.llm_output_tokens,
            list: sample.llm_requests.iter().map(Request::of).collect(),
        }
    }
}

/// One request, by the id its runtime gave it.
#[derive(Serialize)]
pub(crate) struct Request {
    pub(crate) id: String,
    /// `queued`, `prefilling`, `generating` or `completed`.
    pub(crate) state: &'static str,
    pub(crate) prompt_tokens: u64,
    pub(crate) cached_tokens: Option<u64>,
    /// Prompt tokens read so far, while the prompt is being read.
    pub(crate) prefilled_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    /// Its own output speed: the latest interval's while it writes, its
    /// average once completed.
    pub(crate) tok_per_s: Option<f64>,
    pub(crate) first_token_ms: Option<u64>,
    /// How long it has gone without a token, where the runtime says.
    pub(crate) idle_s: Option<f64>,
}

impl Request {
    fn of(request: &crate::domain::RequestUsage) -> Self {
        Self {
            id: request.id.clone(),
            state: if request.completed {
                "completed"
            } else if request.queued {
                "queued"
            } else if request.prefilled.is_some() {
                "prefilling"
            } else {
                "generating"
            },
            prompt_tokens: request.prompt,
            cached_tokens: request.cached,
            prefilled_tokens: request.prefilled,
            output_tokens: request.output,
            tok_per_s: request.output_tps.and_then(tok_per_s),
            first_token_ms: request.ttft_ms,
            idle_s: request.idle_for.map(|idle| round(idle.as_secs_f64(), 3)),
        }
    }
}

/// What the runtime's prompt cache saves.
#[derive(Serialize)]
pub(crate) struct Cache {
    /// The share of prompt tokens served from cache since the runtime
    /// started, and over the last interval.
    pub(crate) reuse_ratio: Option<f64>,
    pub(crate) recent_reuse_ratio: Option<f64>,
    pub(crate) prefix_hit_ratio: Option<f64>,
    /// Its own totals: prompt tokens sent, and those served from cache.
    pub(crate) prompt_tokens_total: Option<u64>,
    pub(crate) cached_tokens_total: Option<u64>,
    /// Its prompt cache on disk (oMLX).
    pub(crate) ssd_used_bytes: Option<u64>,
    pub(crate) ssd_capacity_bytes: Option<u64>,
}

impl Cache {
    fn of(sample: &Sample) -> Self {
        let share = |percent: Option<f64>| percent.and_then(|percent| ratio(percent / 100.0));
        Self {
            reuse_ratio: share(sample.llm_cache_efficiency),
            recent_reuse_ratio: share(sample.llm_cache_interval_efficiency),
            prefix_hit_ratio: share(sample.llm_prefix_hit_rate),
            prompt_tokens_total: sample.llm_total_prompt_tokens,
            cached_tokens_total: sample.llm_total_cached_tokens,
            ssd_used_bytes: sample.ssd_cache.map(|cache| cache.used),
            ssd_capacity_bytes: sample.ssd_cache.map(|cache| cache.capacity),
        }
    }
}

/// Speculative decoding.
#[derive(Serialize)]
pub(crate) struct Draft {
    /// The share of drafted tokens accepted since the last poll.
    pub(crate) acceptance_ratio: Option<f64>,
    /// Tokens one verify pass emitted since the last poll: the drafted
    /// tokens it accepted and the model's own.
    pub(crate) tokens_per_pass: Option<f64>,
}

impl Draft {
    fn of(sample: &Sample) -> Self {
        Self {
            acceptance_ratio: sample.llm_draft_acceptance.and_then(ratio),
            // A pass emits at least the token the model adds itself.
            tokens_per_pass: sample
                .llm_tokens_per_pass
                .filter(|tokens| tokens.is_finite() && *tokens >= 1.0)
                .map(|tokens| round(tokens, 2)),
        }
    }
}

/// How close decode runs to what the chip's memory can feed it, counted per
/// verify pass, as Diagnostics says it: an estimate from the chip's
/// published peak and the model's facts. The draft's own reads are left out.
#[derive(Serialize)]
pub(crate) struct Efficiency {
    pub(crate) source: Source,
    /// A draft proposes the tokens a pass verifies, so the share is per
    /// verify pass, not per token.
    pub(crate) draft: bool,
    /// Verify passes a second: the generation speed over tokens per pass.
    pub(crate) verify_passes_per_s: f64,
    /// Tokens a pass emits: the runtime's count with a draft, 1 without.
    pub(crate) tokens_per_pass: f64,
    /// What one pass reads: the weights, the KV cache of the context it
    /// decodes and the sequence's fixed state.
    pub(crate) bytes_per_pass: u64,
    /// The share of the chip's published peak those passes read.
    pub(crate) bandwidth_share_ratio: f64,
    /// The best share decode has reached for similar work this session,
    /// while generation kept its usual speed: unknown with no session
    /// (`--once`) and until the session has enough readings.
    pub(crate) best_bandwidth_share_ratio: Option<f64>,
    /// The speed this machine could not exceed without a draft: one token a
    /// pass at the chip's full peak.
    pub(crate) no_draft_ceiling_tok_per_s: f64,
}

impl Efficiency {
    fn of(sample: &Sample, decode: Option<&Record>) -> Option<Self> {
        let estimate = crate::efficiency::of_sample(sample)?;
        Some(Self {
            source: Source::Estimate,
            draft: estimate.draft,
            verify_passes_per_s: round(estimate.passes, 2),
            tokens_per_pass: round(estimate.tokens_per_pass, 2),
            bytes_per_pass: estimate.bytes_per_pass,
            bandwidth_share_ratio: round(estimate.share, 4),
            best_bandwidth_share_ratio: decode
                .and_then(|decode| decode.best(sample, &estimate))
                .and_then(ratio),
            no_draft_ceiling_tok_per_s: round(estimate.ceiling(), 2),
        })
    }
}

/// How much more context fits: the tokens each request in flight can still
/// add before the memory its KV cache grows into runs out, or its context
/// window does, as Diagnostics works it out. A forecast.
#[derive(Serialize)]
pub(crate) struct ContextLeft {
    pub(crate) source: Source,
    /// Tokens each request can still add: the smaller of the two limits.
    pub(crate) tokens_per_request: u64,
    /// The requests it is for: those in flight, or with none the next one.
    pub(crate) requests: u64,
    /// What memory alone leaves each request; unknown where only the
    /// window limits it.
    pub(crate) by_memory_tokens: Option<u64>,
    /// What the window alone leaves each request: the window less the most
    /// context one of them holds.
    pub(crate) by_window_tokens: u64,
    /// `memory` or `window`: the limit `tokens_per_request` is (memory's
    /// where they tie, as Diagnostics says it).
    pub(crate) limited_by: &'static str,
}

impl ContextLeft {
    fn of(sample: &Sample, platform: Platform) -> Option<Self> {
        let forecast = crate::context_left::forecast_for(sample, gpu_memory(sample, platform))?;
        let memory = forecast
            .by_memory
            .is_some_and(|memory| memory <= forecast.by_window);
        Some(Self {
            source: Source::Estimate,
            tokens_per_request: forecast.tokens,
            requests: forecast.requests,
            by_memory_tokens: forecast.by_memory,
            by_window_tokens: forecast.by_window,
            limited_by: if memory { "memory" } else { "window" },
        })
    }
}

/// The speed earlier samples of similar work ran at, and below which this
/// work is slow.
#[derive(Serialize)]
pub(crate) struct UsualSpeed {
    pub(crate) source: Source,
    /// `generation` or `prefill`.
    pub(crate) phase: &'static str,
    pub(crate) tok_per_s: Option<f64>,
    pub(crate) slow_below_tok_per_s: Option<f64>,
    /// The work it is the usual for: `1 request · 41.0k context`.
    pub(crate) work: Option<String>,
    /// This sample's speed against it, as a relative change: -0.5 is half
    /// as fast, 0 the usual. In a slowdown, against the speed it fell from.
    pub(crate) relative_change: Option<f64>,
}

impl UsualSpeed {
    fn of(sample: &Sample) -> Option<Self> {
        let insight = &sample.correlation;
        let (usual, slow_below) = insight.usual_speed?;
        Some(Self {
            source: Source::Estimate,
            phase: insight.phase.word(),
            tok_per_s: tok_per_s(usual),
            slow_below_tok_per_s: tok_per_s(slow_below),
            work: Some(insight.usual_for.clone()).filter(|work| !work.is_empty()),
            relative_change: insight
                .delta_percent
                .filter(|percent| percent.is_finite())
                .map(|percent| round(percent / 100.0, 4)),
        })
    }
}

/// A signal that is unusual in this sample.
#[derive(Serialize)]
pub(crate) struct Unusual {
    pub(crate) cause: &'static str,
    /// It beside its usual value, in words.
    pub(crate) note: Option<String>,
}

/// A slowdown or stall under way: speed unusually far below the usual for
/// similar work, three samples running, or an answer with no new token.
#[derive(Serialize)]
pub(crate) struct Slowdown {
    pub(crate) source: Source,
    /// `generation` or `prefill`.
    pub(crate) phase: &'static str,
    pub(crate) stalled: bool,
    /// The reason it is given: what else was unusual, or `none`.
    pub(crate) cause: &'static str,
    /// The reason in words, with its usual value.
    pub(crate) reason: Option<String>,
    /// The speed it fell from, and the slowest it has been.
    pub(crate) usual_tok_per_s: Option<f64>,
    pub(crate) lowest_tok_per_s: Option<f64>,
    pub(crate) duration_s: Option<f64>,
    pub(crate) samples: Option<u32>,
    /// How long the stalled answer has gone without a token.
    pub(crate) stalled_s: Option<f64>,
}

impl Slowdown {
    fn of(sample: &Sample) -> Option<Self> {
        let insight = &sample.correlation;
        if !insight.is_material_drop() {
            return None;
        }
        let episode = insight.episode;
        Some(Self {
            source: Source::Estimate,
            phase: insight.phase.word(),
            stalled: insight.cause == CorrelationCause::Stall,
            cause: cause_name(insight.cause),
            reason: Some(insight.lead.clone()).filter(|lead| !lead.is_empty()),
            usual_tok_per_s: episode.and_then(|episode| tok_per_s(episode.reference)),
            lowest_tok_per_s: episode.and_then(|episode| tok_per_s(episode.low)),
            duration_s: episode.map(|episode| round(episode.seconds, 3)),
            samples: episode.map(|episode| episode.samples),
            stalled_s: insight.stall_seconds.map(|seconds| round(seconds, 3)),
        })
    }
}

/// How a slowdown ended, on the sample where it did.
#[derive(Serialize)]
pub(crate) struct SlowdownEnd {
    pub(crate) source: Source,
    /// `generation` or `prefill`.
    pub(crate) phase: &'static str,
    /// `recovered` (speed came back), `settled` (the slower speed held
    /// with nothing else unusual and is now the usual) or `finished` (the
    /// request ended while slow).
    pub(crate) how: &'static str,
    pub(crate) duration_s: f64,
    pub(crate) lowest_tok_per_s: Option<f64>,
    /// The speed it ended at, and the speed it had fallen from.
    pub(crate) tok_per_s: Option<f64>,
    pub(crate) usual_tok_per_s: Option<f64>,
}

impl SlowdownEnd {
    fn of(end: DropEnd, phase: Phase) -> Self {
        Self {
            source: Source::Estimate,
            phase: phase.word(),
            how: if end.finished {
                "finished"
            } else if end.settled {
                "settled"
            } else {
                "recovered"
            },
            duration_s: round(end.seconds, 3),
            lowest_tok_per_s: tok_per_s(end.low),
            tok_per_s: tok_per_s(end.rate),
            usual_tok_per_s: tok_per_s(end.reference),
        }
    }
}

/// A reason by name. Every cause has one: a cause added to the engine
/// does not compile until it is named here.
pub(crate) fn cause_name(cause: CorrelationCause) -> &'static str {
    match cause {
        CorrelationCause::None => "none",
        CorrelationCause::Stall => "stall",
        CorrelationCause::Paging => "paging",
        CorrelationCause::Compression => "compression",
        CorrelationCause::MemoryPressure => "memory_pressure",
        CorrelationCause::Thermal => "thermal",
        CorrelationCause::MetalMemory => "metal_memory",
        CorrelationCause::Queueing => "queueing",
        CorrelationCause::ConcurrentPrefill => "concurrent_prefill",
        CorrelationCause::ModelMemory => "model_memory",
        CorrelationCause::DiskIo => "disk_io",
        CorrelationCause::Cpu => "cpu",
        CorrelationCause::GpuIdle => "gpu_idle",
        CorrelationCause::DraftAcceptance => "draft_acceptance",
        CorrelationCause::KvPreemption => "kv_preemption",
    }
}

/// Where the GPU's memory comes from on a `platform` machine: decided by
/// the platform the sample was taken on, as the screen decides it by the
/// platform it runs on.
fn gpu_memory(sample: &Sample, platform: Platform) -> crate::gpu::Memory {
    match platform {
        Platform::MacOs => crate::gpu::Memory::Shared,
        _ => crate::gpu::memory(&sample.gpus),
    }
}

/// A size the operating system gave, where 0 means it gave none.
fn known(value: u64) -> Option<u64> {
    (value > 0).then_some(value)
}

fn percent_ratio(percent: u8) -> f64 {
    round(f64::from(percent) / 100.0, 4)
}

/// A share from 0 to 1, or unknown when it is not one.
fn ratio(value: f64) -> Option<f64> {
    (0.0..=1.0).contains(&value).then(|| round(value, 4))
}

fn tok_per_s(value: f64) -> Option<f64> {
    (value.is_finite() && value >= 0.0).then(|| round(value, 2))
}

fn round(value: f64, places: i32) -> f64 {
    let scale = 10_f64.powi(places);
    (value * scale).round() / scale
}

/// One snapshot as a line of JSON.
pub(crate) fn write_line(out: &mut dyn Write, snapshot: &Snapshot) -> io::Result<()> {
    let mut line = serde_json::to_vec(snapshot).map_err(io::Error::other)?;
    line.push(b'\n');
    out.write_all(&line)?;
    out.flush()
}

/// `mlxtop stream`: a snapshot of every sample, one line of JSON each, at
/// `interval`, until the reader goes away or `stop` says so; both end it
/// without an error. A first sample, not written, starts the rates, so
/// every line has them. Each is judged beside the collector's session, as
/// the dashboard judges it.
pub(crate) fn stream(
    collector: &mut Collector,
    interval: Duration,
    out: &mut dyn Write,
    stop: &dyn Fn() -> bool,
) -> io::Result<()> {
    // Rates a runtime reports are taken over spans that follow the
    // interval, as the dashboard's sampler sets them.
    crate::providers::set_poll_interval(interval);
    collector.sample();
    let mut next = Instant::now() + interval;
    loop {
        // Wait for the next sample a little at a time: a signal ends the
        // stream within one look, not one interval.
        loop {
            if stop() {
                return Ok(());
            }
            let wait = next.saturating_duration_since(Instant::now());
            if wait.is_zero() {
                break;
            }
            std::thread::sleep(wait.min(STOP_CHECK));
        }
        let started = Instant::now();
        let sample = collector.sample();
        match write_line(
            out,
            &session_snapshot(collector, &sample, interval.as_secs()),
        ) {
            // The reader went away (`| head -3`): the stream is done.
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
            result => result?,
        }
        // A fixed rate: "every 1s" means a line each second.
        next = (started + interval).max(Instant::now());
    }
}

#[cfg(test)]
#[path = "tests/export.rs"]
pub(crate) mod tests;
