// SPDX-License-Identifier: MIT
//! macOS, Linux and Windows host-counter collection through the injectable Host interface.
use crate::config::Thresholds;
use crate::domain::{ChartMetric, MetalTelemetry, Sample, Tone, VmCounters};
use crate::gpu;
use crate::host::{Host, WindowsReading};
use crate::parsing::{
    find_named_number, find_named_numbers, find_named_string, find_number, parse_unit,
};
use std::path::Path;
pub(crate) fn sample_macos_memory(host: &dyn Host, sample: &mut Sample, page_size: u64) {
    let level = host
        .command(
            "/usr/sbin/sysctl",
            &["-n", "kern.memorystatus_vm_pressure_level"],
        )
        .unwrap_or_default();
    match level.trim() {
        "1" => {
            sample.pressure = "GREEN".into();
            sample.pressure_meaning = "normal".into();
            sample.pressure_tone = Tone::Green;
        }
        "2" => {
            sample.pressure = "YELLOW".into();
            sample.pressure_meaning = "warning".into();
            sample.pressure_tone = Tone::Yellow;
        }
        "4" => {
            sample.pressure = "RED".into();
            sample.pressure_meaning = "critical".into();
            sample.pressure_tone = Tone::Red;
        }
        _ => {}
    }

    if let Some(output) = host.command("/usr/bin/memory_pressure", &["-Q"]) {
        if let Some(line) = output.lines().find(|line| line.contains("free percentage")) {
            sample.availability = line
                .split(|c: char| !c.is_ascii_digit())
                .find(|v| !v.is_empty())
                .and_then(|v| v.parse::<u8>().ok());
        }
    }

    let vm = host.command("/usr/bin/vm_stat", &[]).unwrap_or_default();
    sample.vm_available = !vm.trim().is_empty();
    let counters = parse_vm_stat(&vm, page_size);
    // vm_stat prints free_count minus speculative_count as "Pages free".
    // memory_pressure -Q uses AVAILABLE_NON_COMPRESSED_MEMORY, including
    // active and inactive application pages, so it cannot measure RAM use.
    sample.resident_memory = resident_memory_bytes(
        sample.total_memory,
        counters
            .free
            .zip(counters.speculative)
            .and_then(|(free, speculative)| free.checked_add(speculative)),
    );
    sample.wired = counters.wired;
    sample.compressor = counters.compressor;
    sample.compressed_logical = counters.compressed_logical;
    sample.anonymous = counters.anonymous;
    sample.file_backed = counters.file_backed;
    let swap_usage = host.command("/usr/sbin/sysctl", &["-n", "vm.swapusage"]);
    sample.swap_available = swap_usage.is_some();
    (sample.swap_total, sample.swap_used) = parse_swap_usage(&swap_usage.unwrap_or_default());
}

pub(crate) fn macos_counters_for_rates(host: &dyn Host, page_size: u64) -> VmCounters {
    let vm = host.command("/usr/bin/vm_stat", &[]).unwrap_or_default();
    parse_vm_stat(&vm, page_size)
}

pub(crate) fn sample_macos_gpu_thermal(host: &dyn Host, sample: &mut Sample) {
    let ioreg = host
        .command(
            "/usr/sbin/ioreg",
            &["-r", "-d", "1", "-w", "0", "-c", "IOAccelerator"],
        )
        .unwrap_or_default();
    (sample.gpu_util, sample.gpu_alloc, sample.gpu_in_use) = parse_gpu(&ioreg);
    let live_metal = parse_metal_hardware(&ioreg);
    sample.metal.device_name = live_metal.device_name.or(sample.metal.device_name.take());
    sample.metal.gpu_cores = live_metal.gpu_cores.or(sample.metal.gpu_cores);
    sample.metal.renderer_util = live_metal.renderer_util;
    sample.metal.tiler_util = live_metal.tiler_util;
    sample.thermal = parse_thermal(
        &host
            .command("/usr/bin/pmset", &["-g", "therm"])
            .unwrap_or_default(),
    );
}

// --- Linux sampling -------------------------------------------------------
// Linux has no vm_stat / ioreg / pmset. Memory and swap come from
// /proc/meminfo, paging rates from /proc/vmstat (pswpin/pswpout), pressure
// level from the MemAvailable ratio blended with /proc/pressure/memory
// stalls, GPUs from nvidia-smi when present, and thermals from
// /sys/class/thermal. Anything without a source stays `None`/unavailable
// and the UI already renders that as a dash.

#[derive(Default)]
pub(crate) struct LinuxMeminfo {
    pub(crate) total_kb: u64,
    pub(crate) free_kb: Option<u64>,
    pub(crate) available_kb: u64,
    pub(crate) swap_total_kb: u64,
    pub(crate) swap_free_kb: u64,
    pub(crate) anon_kb: u64,
    pub(crate) file_kb: u64,
}

pub(crate) fn parse_meminfo_value_kb(text: &str, key: &str) -> u64 {
    parse_meminfo_optional_kb(text, key).unwrap_or(0)
}

pub(crate) fn parse_meminfo_optional_kb(text: &str, key: &str) -> Option<u64> {
    text.lines()
        .find(|line| line.starts_with(key))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u64>().ok())
}

pub(crate) fn parse_linux_meminfo(text: &str) -> LinuxMeminfo {
    let total_kb = parse_meminfo_value_kb(text, "MemTotal:");
    let available_kb = parse_meminfo_value_kb(text, "MemAvailable:");
    let swap_total_kb = parse_meminfo_value_kb(text, "SwapTotal:");
    let swap_free_kb = parse_meminfo_value_kb(text, "SwapFree:");
    let anon_kb = parse_meminfo_value_kb(text, "Active(anon):")
        .saturating_add(parse_meminfo_value_kb(text, "Inactive(anon):"));
    let anon_fallback = if anon_kb == 0 {
        parse_meminfo_value_kb(text, "AnonPages:")
    } else {
        anon_kb
    };
    let file_kb = parse_meminfo_value_kb(text, "Active(file):")
        .saturating_add(parse_meminfo_value_kb(text, "Inactive(file):"))
        .saturating_add(parse_meminfo_value_kb(text, "Cached:"))
        .saturating_add(parse_meminfo_value_kb(text, "Buffers:"));
    LinuxMeminfo {
        total_kb,
        free_kb: parse_meminfo_optional_kb(text, "MemFree:"),
        available_kb,
        swap_total_kb,
        swap_free_kb,
        anon_kb: anon_fallback,
        file_kb,
    }
}

pub(crate) fn parse_linux_vmstat_value(text: &str, key: &str) -> u64 {
    text.lines()
        .find(|line| line.starts_with(key))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
}

pub(crate) fn parse_linux_paging(text: &str) -> (u64, u64) {
    (
        parse_linux_vmstat_value(text, "pswpin"),
        parse_linux_vmstat_value(text, "pswpout"),
    )
}

/// Stall average (`avg10`) of the `full` line in /proc/pressure/memory.
/// Returns a percentage in the 0–100 range, or `None` when unavailable.
pub(crate) fn parse_memory_pressure_stall(text: &str) -> Option<f64> {
    text.lines()
        .find(|line| line.starts_with("full"))?
        .split_whitespace()
        .find_map(|token| token.strip_prefix("avg10=")?.parse::<f64>().ok())
}

pub(crate) fn linux_pressure_state(
    load_percent: u64,
    stall_avg10: Option<f64>,
    thresholds: Thresholds,
) -> (&'static str, Tone) {
    // MemAvailable excludes reclaimable cache from load; resident occupancy
    // does not. Apply the configured bands here, then escalate for full stalls.
    let mut level = match ChartMetric::Memory.tone(load_percent, thresholds) {
        Tone::Red => 2,
        Tone::Yellow => 1,
        _ => 0,
    };
    match stall_avg10 {
        Some(stall) if stall >= 5.0 => level = level.max(2),
        Some(stall) if stall >= 1.0 => level = level.max(1),
        _ => {}
    }
    match level {
        2 => ("RED", Tone::Red),
        1 => ("YELLOW", Tone::Yellow),
        _ => ("GREEN", Tone::Green),
    }
}

pub(crate) fn linux_total_memory(host: &dyn Host) -> u64 {
    host.read_file(Path::new("/proc/meminfo"))
        .map(|text| parse_linux_meminfo(&text))
        .map(|info| info.total_kb.saturating_mul(1024))
        .unwrap_or(0)
}

pub(crate) fn linux_page_size(host: &dyn Host) -> u64 {
    host.command_u64("getconf", &["PAGESIZE"]).unwrap_or(4096)
}

pub(crate) fn linux_cpu_architecture(host: &dyn Host) -> Option<String> {
    host.command("uname", &["-m"])
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub(crate) fn linux_metal_init(host: &dyn Host) -> MetalTelemetry {
    MetalTelemetry {
        architecture: linux_cpu_architecture(host),
        ..MetalTelemetry::default()
    }
}

pub(crate) fn linux_thermal_celsius(host: &dyn Host, nvidia_temp: Option<u64>) -> Option<u64> {
    let mut hottest = nvidia_temp.unwrap_or(0);
    for entry in host.read_dir(Path::new("/sys/class/thermal")) {
        if let Some(text) = host.read_file(&entry.join("temp")) {
            // Values are millidegrees Celsius on most drivers.
            if let Ok(millidegrees) = text.trim().parse::<i64>() {
                if millidegrees > 0 {
                    hottest = hottest.max((millidegrees / 1000).max(0) as u64);
                }
            }
        }
    }
    (hottest > 0).then_some(hottest)
}

pub(crate) fn linux_thermal_label(hottest: Option<u64>) -> String {
    hottest
        .map(|value| format!("{value}°C measured"))
        .unwrap_or_else(|| "unavailable".into())
}

pub(crate) fn linux_counters_for_rates(host: &dyn Host) -> VmCounters {
    let mut counters = VmCounters::default();
    if let Some(text) = host.read_file(Path::new("/proc/vmstat")) {
        let (swapins, swapouts) = parse_linux_paging(&text);
        counters.swapins = swapins;
        counters.swapouts = swapouts;
    }
    counters
}

pub(crate) fn sample_linux_memory(
    host: &dyn Host,
    sample: &mut Sample,
    _page_size: u64,
    total_memory: u64,
    thresholds: Thresholds,
) {
    let Some(text) = host.read_file(Path::new("/proc/meminfo")) else {
        return;
    };
    let info = parse_linux_meminfo(&text);
    let total = if total_memory > 0 {
        total_memory
    } else {
        info.total_kb.saturating_mul(1024)
    };
    sample.total_memory = total;
    sample.resident_memory =
        resident_memory_bytes(total, info.free_kb.and_then(|free| free.checked_mul(1024)));
    sample.vm_available = true;
    sample.anonymous = info.anon_kb.saturating_mul(1024);
    sample.file_backed = info.file_kb.saturating_mul(1024);
    sample.wired = 0;
    sample.compressor = 0;
    sample.compressed_logical = 0;

    if total > 0 {
        let available = info.available_kb.saturating_mul(1024).min(total);
        let free_percent = (available.saturating_mul(100) / total).min(100);
        sample.availability = u8::try_from(free_percent).ok();
        let load = 100u64.saturating_sub(free_percent);
        let stall = host
            .read_file(Path::new("/proc/pressure/memory"))
            .as_deref()
            .and_then(parse_memory_pressure_stall);
        let (pressure, tone) = linux_pressure_state(load, stall, thresholds);
        sample.pressure = pressure.into();
        sample.pressure_meaning = match pressure {
            "GREEN" => "normal",
            "YELLOW" => "warning",
            "RED" => "critical",
            _ => "unavailable",
        }
        .into();
        sample.pressure_tone = tone;
    }

    sample.swap_total = info.swap_total_kb.saturating_mul(1024);
    sample.swap_used = info
        .swap_total_kb
        .saturating_sub(info.swap_free_kb.min(info.swap_total_kb))
        .saturating_mul(1024);
    sample.swap_available = true;
}

pub(crate) fn sample_linux_gpu_thermal(
    host: &dyn Host,
    sample: &mut Sample,
    previous: &[gpu::Device],
) {
    sample.gpus = gpu::collect(host, previous);
    sample.gpu_util = gpu::peak_utilization(&sample.gpus);
    // VRAM remains per-card. A sum would falsely suggest a single allocation
    // pool and feed NVIDIA memory into the Apple Metal correlation rules.
    sample.gpu_in_use = None;
    sample.gpu_alloc = None;
    sample.metal.renderer_util = None;
    sample.metal.tiler_util = None;
    let hottest_gpu = sample.gpus.iter().filter_map(|gpu| gpu.temperature).max();
    sample.thermal = linux_thermal_label(linux_thermal_celsius(host, hottest_gpu));
}

// --- Windows sampling -----------------------------------------------------
// Memory, commit charge and processes come from the Windows APIs through
// `Host::windows`; GPUs from `nvidia-smi`, which ships with the driver.
// Windows exposes no page-in/page-out counters through those APIs, so paging
// rates stay unavailable rather than reading as a healthy zero.

pub(crate) fn sample_windows_memory(
    reading: Option<&WindowsReading>,
    sample: &mut Sample,
    thresholds: Thresholds,
) {
    sample.paging_unavailable = true;
    let Some(reading) = reading.filter(|reading| reading.total > 0) else {
        return;
    };
    let total = reading.total;
    let available = reading.available.min(total);
    sample.total_memory = total;
    // Windows reports available (free plus standby) memory, not free alone.
    sample.resident_memory = Some(total - available);
    sample.vm_available = true;
    sample.anonymous = total - available;
    sample.file_backed = 0;
    sample.wired = 0;
    sample.compressor = 0;
    sample.compressed_logical = 0;
    let free_percent = (u128::from(available) * 100 / u128::from(total)).min(100) as u64;
    sample.availability = u8::try_from(free_percent).ok();
    let (pressure, tone) = linux_pressure_state(100 - free_percent, None, thresholds);
    sample.pressure = pressure.into();
    sample.pressure_meaning = match pressure {
        "GREEN" => "normal",
        "YELLOW" => "warning",
        _ => "critical",
    }
    .into();
    sample.pressure_tone = tone;
    sample.swap_total = reading.swap_total;
    sample.swap_used = reading.swap_used.min(reading.swap_total);
    sample.swap_available = true;
}

pub(crate) fn sample_windows_gpu_thermal(
    host: &dyn Host,
    sample: &mut Sample,
    previous: &[gpu::Device],
) {
    sample.gpus = gpu::collect(host, previous);
    sample.gpu_util = gpu::peak_utilization(&sample.gpus);
    sample.gpu_in_use = None;
    sample.gpu_alloc = None;
    sample.metal.renderer_util = None;
    sample.metal.tiler_util = None;
    // Windows has no unprivileged CPU temperature source; report the GPUs.
    let hottest_gpu = sample.gpus.iter().filter_map(|gpu| gpu.temperature).max();
    sample.thermal = linux_thermal_label(hottest_gpu);
}

pub(crate) fn parse_vm_stat(text: &str, page_size: u64) -> VmCounters {
    let mut c = VmCounters::default();
    for line in text.lines() {
        let Some(value) = line
            .split(':')
            .nth(1)
            .and_then(|v| {
                v.split_whitespace()
                    .next()
                    .map(|value| value.trim_matches(|c: char| !c.is_ascii_digit()))
            })
            .and_then(|v| v.parse::<u64>().ok())
        else {
            continue;
        };
        let bytes = value.saturating_mul(page_size);
        if line.starts_with("Pages free:") {
            c.free = value.checked_mul(page_size);
        } else if line.starts_with("Pages speculative:") {
            c.speculative = value.checked_mul(page_size);
        } else if line.starts_with("Pages wired") {
            c.wired = bytes;
        } else if line.starts_with("Pages occupied by compressor")
            || line.starts_with("Pages used by compressor")
        {
            c.compressor = bytes;
        } else if line.starts_with("Pages stored in compressor")
            || line.starts_with("Uncompressed pages")
        {
            c.compressed_logical = bytes;
        } else if line.starts_with("Anonymous pages") {
            c.anonymous = bytes;
        } else if line.starts_with("File-backed pages") {
            c.file_backed = bytes;
        } else if line.starts_with("Swapins") || line.contains("\"Swapins\"") {
            c.swapins = value;
        } else if line.starts_with("Swapouts") || line.contains("\"Swapouts\"") {
            c.swapouts = value;
        } else if line.starts_with("Compressions") || line.starts_with("Pages compressed") {
            c.compressions = value;
        } else if line.starts_with("Decompressions") || line.starts_with("Pages decompressed") {
            c.decompressions = value;
        } else if line.starts_with("Pages reactivated") {
            c.reactivations = value;
        }
    }
    c
}

pub(crate) fn resident_memory_bytes(total: u64, free: Option<u64>) -> Option<u64> {
    (total > 0).then_some(())?;
    total.checked_sub(free?)
}

pub(crate) fn resident_memory_percent(sample: &Sample) -> Option<u64> {
    let used = sample.resident_memory?;
    if sample.total_memory == 0 || used > sample.total_memory {
        return None;
    }
    Some((u128::from(used) * 100 / u128::from(sample.total_memory)) as u64)
}

pub(crate) fn parse_swap_usage(text: &str) -> (u64, u64) {
    let mut total = 0;
    let mut used = 0;
    let normalized = text.replace('=', " = ");
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        if (tokens[i] == "total" || tokens[i] == "used") && i + 1 < tokens.len() {
            let value_index = if tokens[i + 1] == "=" { i + 2 } else { i + 1 };
            if let Some(value) = tokens.get(value_index) {
                if tokens[i] == "total" {
                    total = parse_unit(value);
                } else {
                    used = parse_unit(value);
                }
                i = value_index;
            }
        }
        i += 1;
    }
    (total, used)
}

pub(crate) fn parse_gpu(text: &str) -> (Option<u8>, Option<u64>, Option<u64>) {
    (
        find_named_number(text, "Device Utilization %")
            .or_else(|| find_named_number(text, "Renderer Utilization %"))
            .map(|v| v as u8),
        find_named_numbers(text, "Alloc system memory")
            .into_iter()
            .next_back(),
        find_named_numbers(text, "In use system memory")
            .into_iter()
            .next_back(),
    )
}

pub(crate) fn parse_metal_hardware(text: &str) -> MetalTelemetry {
    MetalTelemetry {
        device_name: find_named_string(text, "model")
            .or_else(|| find_named_string(text, "MetalPluginName")),
        architecture: None,
        gpu_cores: find_named_number(text, "gpu-core-count")
            .and_then(|value| value.try_into().ok()),
        renderer_util: find_named_number(text, "Renderer Utilization %")
            .and_then(|value| value.try_into().ok()),
        tiler_util: find_named_number(text, "Tiler Utilization %")
            .and_then(|value| value.try_into().ok()),
        resource_limit: None,
    }
}

pub(crate) fn parse_thermal(text: &str) -> String {
    if let Some(value) = find_number(text, "CPU_Speed_Limit") {
        if value < 100 {
            return format!("limited {value}%");
        }
        return "no limit".into();
    }
    if text.is_empty() {
        "unavailable".into()
    } else if text.contains("No thermal warning") || text.contains("No performance warning") {
        "no warning".into()
    } else {
        "warning reported".into()
    }
}
