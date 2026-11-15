// SPDX-License-Identifier: MIT
//! Per-device discrete GPU counters. Device memory is separate from Apple Metal accounting.
//! NVIDIA cards are collected on Linux and Windows, where `nvidia-smi` ships with the
//! driver; AMD cards on Linux from the amdgpu driver's sysfs files (`gpu_amd`).

use crate::domain::MIB;
use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::host::Host;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Device {
    pub index: u32,
    pub uuid: String,
    pub vendor: Vendor,
    pub name: String,
    pub utilization: Option<u8>,
    pub used: Option<u64>,
    pub total: Option<u64>,
    pub temperature: Option<u64>,
    /// Active clock-limit reasons as the NVML bitmask; `None` when the driver
    /// does not report them.
    pub throttle_reasons: Option<u64>,
    /// The GPU has no memory of its own and works in system RAM: an AMD APU
    /// such as Strix Halo, or an NVIDIA integrated GPU such as DGX Spark's
    /// GB10, which reports no device memory. `used` and `total` are then
    /// unknown, and the RAM it holds is in `limits`.
    #[serde(default)]
    pub unified: bool,
    /// Power, clocks, memory traffic, the PCIe link and the RAM it holds.
    #[serde(default)]
    pub limits: Limits,
}

/// What a card works against: the power it draws and its cap, its clock and
/// the clock's maximum, how busy its memory is, its PCIe link, and the system
/// RAM it holds. Each reading is unknown until the driver reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct Limits {
    /// Power the card draws, and the cap the driver holds it under, in
    /// milliwatts. On an AMD APU the reading is the whole chip, CPU included.
    pub power_mw: Option<u64>,
    pub power_cap_mw: Option<u64>,
    /// Graphics (SM) clock and its maximum, in MHz.
    pub clock_mhz: Option<u32>,
    pub clock_max_mhz: Option<u32>,
    /// The share of time the card's memory was read or written, in percent:
    /// near 100 while decoding means memory bandwidth is the limit.
    pub memory_busy: Option<u8>,
    /// PCIe link generation and lanes now, and the most this card and its
    /// slot allow. A link slows down when the card idles.
    pub link_gen: Option<u8>,
    pub link_gen_max: Option<u8>,
    pub link_width: Option<u8>,
    pub link_width_max: Option<u8>,
    /// System RAM the GPU holds, and the most it may hold: AMD's GTT (with
    /// an APU's BIOS carve-out, which is RAM too), or on an integrated NVIDIA
    /// GPU what its processes hold.
    pub ram_used: Option<u64>,
    pub ram_limit: Option<u64>,
    /// GPU memory held by each process, as (PID, bytes), largest first. Read
    /// on Linux only where it answers a question: an integrated GPU, whose
    /// device memory is not reported, or a card nearly full.
    pub processes: Vec<(u32, u64)>,
    /// When `nvidia-smi` is next asked for these: every few seconds, carried
    /// between reads, or a minute after a query that failed outright.
    #[serde(skip)]
    pub next_read: Option<Instant>,
}

/// The backend that reported a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Vendor {
    Nvidia,
    Amd,
}

impl Vendor {
    pub fn label(self) -> &'static str {
        match self {
            Self::Nvidia => "NVIDIA",
            Self::Amd => "AMD",
        }
    }

    /// Where the counters come from, for the panel footer.
    pub fn source(self) -> &'static str {
        match self {
            Self::Nvidia => "nvidia-smi",
            Self::Amd => "amdgpu sysfs",
        }
    }
}

/// Why the driver is holding clocks below their maximum, most severe first.
/// Idle, application-clock and sync-boost reasons are normal and excluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Throttle {
    Thermal,
    PowerBrake,
    HardwareSlowdown,
    PowerCap,
}

impl Throttle {
    pub fn label(self) -> &'static str {
        match self {
            Self::Thermal => "thermal slowdown",
            Self::PowerBrake => "power brake",
            Self::HardwareSlowdown => "hardware slowdown",
            Self::PowerCap => "power cap",
        }
    }

    /// Fits the 13-column STATE cell in the GPU panel.
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Thermal => "thermal limit",
            Self::PowerBrake => "power brake",
            Self::HardwareSlowdown => "hw slowdown",
            Self::PowerCap => "power cap",
        }
    }

    /// A power cap under load is the card working as configured; the other
    /// reasons cost speed the configured card would otherwise have.
    pub fn is_fault(self) -> bool {
        self != Self::PowerCap
    }
}

// NVML clocksEventReasons / clocksThrottleReasons bits.
const SW_POWER_CAP: u64 = 0x4;
const HW_SLOWDOWN: u64 = 0x8;
const SW_THERMAL_SLOWDOWN: u64 = 0x20;
const HW_THERMAL_SLOWDOWN: u64 = 0x40;
const HW_POWER_BRAKE_SLOWDOWN: u64 = 0x80;

/// A card this busy is working; below it, held clocks and a slowed link are
/// the driver saving power, not a limit.
pub(crate) const BUSY_PERCENT: u8 = 20;

/// A busy card whose clock is under two thirds of its maximum is held down:
/// boost keeps a working card near its top clock, and NVIDIA's hardware
/// slowdowns halve it.
const CLOCK_HELD_PERCENT: u32 = 67;

impl Device {
    pub fn memory_percent(&self) -> Option<u16> {
        let (used, total) = self.used.zip(self.total)?;
        (total > 0).then(|| ((u128::from(used) * 100 / u128::from(total)).min(100)) as u16)
    }

    /// The share of the most system RAM it may hold that the GPU holds.
    pub fn ram_percent(&self) -> Option<u16> {
        let (used, limit) = self.limits.ram_used.zip(self.limits.ram_limit)?;
        (limit > 0).then(|| ((u128::from(used) * 100 / u128::from(limit)).min(100)) as u16)
    }

    pub fn throttle(&self) -> Option<Throttle> {
        let reasons = self.throttle_reasons?;
        [
            (SW_THERMAL_SLOWDOWN | HW_THERMAL_SLOWDOWN, Throttle::Thermal),
            (HW_POWER_BRAKE_SLOWDOWN, Throttle::PowerBrake),
            (HW_SLOWDOWN, Throttle::HardwareSlowdown),
            (SW_POWER_CAP, Throttle::PowerCap),
        ]
        .into_iter()
        .find_map(|(bits, cause)| (reasons & bits != 0).then_some(cause))
    }

    /// Working hard enough that a held clock or a slow link costs speed.
    pub fn busy(&self) -> bool {
        self.utilization.is_some_and(|load| load >= BUSY_PERCENT)
    }

    /// The clock as a share of its maximum, while the card is busy and that
    /// share is low enough to mean the clock is held down.
    pub fn clock_held(&self) -> Option<u32> {
        let (clock, max) = self.limits.clock_mhz.zip(self.limits.clock_max_mhz)?;
        let share = clock.saturating_mul(100) / max.max(1);
        (self.busy() && share < CLOCK_HELD_PERCENT).then_some(share)
    }

    /// A busy card's PCIe link below what the card and its slot allow. An
    /// idle card slows its link on purpose; an integrated GPU has no slot.
    pub fn link_reduced(&self) -> bool {
        let limits = &self.limits;
        let below = |now: Option<u8>, max: Option<u8>| now.zip(max).is_some_and(|(n, m)| n < m);
        self.busy()
            && !self.unified
            && (below(limits.link_gen, limits.link_gen_max)
                || below(limits.link_width, limits.link_width_max))
    }

    fn unavailable(&self) -> Self {
        Self {
            utilization: None,
            used: None,
            total: None,
            temperature: None,
            throttle_reasons: None,
            limits: Limits::default(),
            ..self.clone()
        }
    }
}

/// Older drivers name the field `clocks_throttle_reasons`; drivers from the
/// 535 series also accept `clocks_event_reasons`. An unknown field fails the
/// whole query, so it runs apart from the core counters.
const THROTTLE_FIELDS: [&str; 2] = [
    "--query-gpu=uuid,clocks_throttle_reasons.active",
    "--query-gpu=uuid,clocks_event_reasons.active",
];

/// The counters every driver answers, read every poll.
const CORE_FIELDS: &str =
    "--query-gpu=index,uuid,name,utilization.gpu,memory.used,memory.total,temperature.gpu";

/// Power, clocks, memory traffic and the PCIe link, in a query of their own:
/// one field a driver does not know fails the whole query, and the core
/// counters must survive that.
const LIMIT_FIELDS: &str = "--query-gpu=uuid,power.draw,enforced.power.limit,clocks.sm,clocks.max.sm,utilization.memory,pcie.link.gen.current,pcie.link.gen.max,pcie.link.width.current,pcie.link.width.max";

/// GPU memory per process. On an integrated GPU `used_memory` is the system
/// RAM the process holds; the device total is not reported there.
const PROCESS_FIELDS: &str = "--query-compute-apps=gpu_uuid,pid,used_memory";

const CSV: &str = "--format=csv,noheader,nounits";

/// Power, clocks and the link change over seconds, and each read costs an
/// `nvidia-smi` process: they are read this often and carried between.
pub(crate) const LIMITS_INTERVAL: Duration = Duration::from_secs(5);
/// A query that fails outright (a driver without one of the fields) is
/// retried this rarely, so its warning does not fill the log.
pub(crate) const LIMITS_RETRY: Duration = Duration::from_secs(60);

/// NVIDIA cards first, then AMD cards, numbered in that order.
pub(crate) fn collect(host: &dyn Host, previous: &[Device]) -> Vec<Device> {
    let (amd_before, nvidia_before): (Vec<Device>, Vec<Device>) = previous
        .iter()
        .cloned()
        .partition(|gpu| gpu.vendor == Vendor::Amd);
    merge(
        collect_nvidia(host, &nvidia_before),
        crate::gpu_amd::collect(host, &amd_before),
    )
}

/// One list of cards, NVIDIA first. An AMD APU is listed only on a machine
/// without a card of its own: beside one it drives the desktop, and the RAM
/// it holds would muddle the card's VRAM.
pub(crate) fn merge(mut devices: Vec<Device>, mut amd: Vec<Device>) -> Vec<Device> {
    if devices.iter().chain(&amd).any(|gpu| !gpu.unified) {
        amd.retain(|gpu| !gpu.unified);
    }
    let offset = devices.len() as u32;
    devices.extend(amd.into_iter().enumerate().map(|(index, gpu)| Device {
        index: offset + index as u32,
        ..gpu
    }));
    devices
}

fn collect_nvidia(host: &dyn Host, previous: &[Device]) -> Vec<Device> {
    if !cfg!(any(target_os = "linux", target_os = "windows")) {
        return Vec::new();
    }
    // Windows' display driver model keeps per-process memory to itself.
    nvidia(host, previous, cfg!(target_os = "linux"), Instant::now())
}

/// Query all physical devices in one invocation. UUIDs distinguish identical
/// models and survive driver enumeration changes; indices remain familiar
/// labels. Clock-limit reasons follow every poll; power, clocks and the link
/// every `LIMITS_INTERVAL`, each in a query of its own.
pub(crate) fn nvidia(
    host: &dyn Host,
    previous: &[Device],
    per_process: bool,
    now: Instant,
) -> Vec<Device> {
    let output = host.command("nvidia-smi", &[CORE_FIELDS, CSV]);
    let mut devices = readings(output.as_deref(), previous);
    if !devices.iter().any(|gpu| gpu.utilization.is_some()) {
        return devices;
    }
    let reasons = THROTTLE_FIELDS
        .iter()
        .find_map(|fields| host.command("nvidia-smi", &[fields, "--format=csv,noheader"]));
    if let Some(text) = reasons {
        apply_throttle_reasons(&mut devices, &text);
    }
    let before = |gpu: &Device| previous.iter().find(|before| before.uuid == gpu.uuid);
    let due = devices.iter().any(|gpu| {
        before(gpu)
            .and_then(|before| before.limits.next_read)
            .is_none_or(|at| now >= at)
    });
    if !due {
        for device in &mut devices {
            if let Some(before) = previous.iter().find(|before| before.uuid == device.uuid) {
                device.limits = before.limits.clone();
            }
        }
        return devices;
    }
    let limits = host.command("nvidia-smi", &[LIMIT_FIELDS, CSV]);
    apply_limits(&mut devices, limits.as_deref().unwrap_or_default(), now);
    if limits.is_none() {
        for device in &mut devices {
            device.limits.next_read = Some(now + LIMITS_RETRY);
        }
    }
    let wanted = |gpu: &Device| {
        gpu.unified
            || gpu
                .memory_percent()
                .is_some_and(|percent| percent >= crate::gpu_findings::VRAM_FULL_EXIT)
    };
    if per_process && devices.iter().any(wanted) {
        if let Some(text) = host.command("nvidia-smi", &[PROCESS_FIELDS, CSV]) {
            apply_processes(&mut devices, &text);
            // One query lists every card; each keeps the list only where it
            // answers a question.
            for device in devices.iter_mut().filter(|gpu| !wanted(gpu)) {
                device.limits.processes.clear();
            }
        }
    }
    devices
}

/// Attach `uuid, 0x…` rows to the cards they name; other cards stay unknown.
pub(crate) fn apply_throttle_reasons(devices: &mut [Device], text: &str) {
    for line in text.lines() {
        let Some((uuid, mask)) = line.split_once(',') else {
            continue;
        };
        let mask = mask.trim();
        let Some(reasons) = mask
            .strip_prefix("0x")
            .and_then(|hex| u64::from_str_radix(hex, 16).ok())
        else {
            continue;
        };
        if let Some(device) = devices.iter_mut().find(|gpu| gpu.uuid == uuid.trim()) {
            device.throttle_reasons = Some(reasons);
        }
    }
}

/// Attach the power, clock, memory and link row of each card. Every card
/// counts as read now, so a card the driver answered nothing for stays
/// unknown until the next read rather than being asked again every poll.
/// `[N/A]`, `[Not Supported]` and anything else that is not a number read
/// as unknown, field by field.
pub(crate) fn apply_limits(devices: &mut [Device], text: &str, now: Instant) {
    for device in devices.iter_mut() {
        device.limits = Limits {
            next_read: Some(now + LIMITS_INTERVAL),
            ..Limits::default()
        };
    }
    for line in text.lines() {
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        let [uuid, power, cap, clock, clock_max, busy, generation, generation_max, width, width_max] =
            fields.as_slice()
        else {
            continue;
        };
        let Some(device) = devices.iter_mut().find(|gpu| gpu.uuid == *uuid) else {
            continue;
        };
        let limits = &mut device.limits;
        limits.power_mw = watts(power);
        limits.power_cap_mw = watts(cap);
        limits.clock_mhz = within(clock, 1, 20_000);
        limits.clock_max_mhz = within(clock_max, 1, 20_000);
        limits.memory_busy = within(busy, 0, 100).map(|percent: u32| percent as u8);
        limits.link_gen = within(generation, 1, 9).map(|n: u32| n as u8);
        limits.link_gen_max = within(generation_max, 1, 9).map(|n: u32| n as u8);
        limits.link_width = within(width, 1, 64).map(|n: u32| n as u8);
        limits.link_width_max = within(width_max, 1, 64).map(|n: u32| n as u8);
    }
}

/// Attach `uuid, pid, MiB` rows to the cards they name. On an integrated
/// GPU what its processes hold is the RAM it holds; one process whose
/// memory is not reported leaves that sum unknown.
pub(crate) fn apply_processes(devices: &mut [Device], text: &str) {
    let rows: Vec<(&str, Option<u32>, Option<u64>)> = text
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(',').map(str::trim);
            let uuid = fields.next()?;
            let pid = fields.next()?.parse().ok();
            let used = number(fields.next()?).and_then(|mib| mib.checked_mul(MIB));
            fields.next().is_none().then_some((uuid, pid, used))
        })
        .collect();
    for device in devices.iter_mut() {
        let mine: Vec<(Option<u32>, Option<u64>)> = rows
            .iter()
            .filter(|(uuid, ..)| *uuid == device.uuid)
            .map(|(_, pid, used)| (*pid, *used))
            .collect();
        let mut processes: Vec<(u32, u64)> = mine
            .iter()
            .filter_map(|(pid, used)| pid.zip(*used))
            .collect();
        processes.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        if device.unified {
            device.limits.ram_used = mine.iter().try_fold(0_u64, |sum, (pid, used)| {
                pid.and(*used).and_then(|used| sum.checked_add(used))
            });
        }
        device.limits.processes = processes;
    }
}

/// Watts as milliwatts. A board never draws nothing, so zero is unknown too.
fn watts(field: &str) -> Option<u64> {
    let value: f64 = field.parse().ok()?;
    (value.is_finite() && value > 0.0 && value < 100_000.0).then(|| (value * 1000.0).round() as u64)
}

/// A whole number from `low` to `high`, else unknown.
fn within(field: &str, low: u32, high: u32) -> Option<u32> {
    field
        .parse()
        .ok()
        .filter(|value| (low..=high).contains(value))
}

fn readings(output: Option<&str>, previous: &[Device]) -> Vec<Device> {
    let devices = output.map(parse).unwrap_or_default();
    if devices.is_empty() {
        // A failed/invalid read is not a healthy zero, and must not erase the
        // panel. Retain identity only, never the last successful counters.
        previous.iter().map(Device::unavailable).collect()
    } else {
        devices
    }
}

pub(crate) fn parse(text: &str) -> Vec<Device> {
    let mut seen = HashSet::new();
    let mut devices: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let (index, rest) = line.split_once(',')?;
            let index = index.trim().parse().ok()?;
            let (uuid, rest) = rest.split_once(',')?;
            let uuid = uuid.trim();
            if !uuid.starts_with("GPU-") || uuid.chars().any(char::is_control) {
                return None;
            }
            // The last four fields are numeric. Splitting from the right also
            // preserves a quoted device name containing commas.
            let mut fields = rest.rsplitn(5, ',');
            let temperature = number(fields.next()?);
            let total = number(fields.next()?).and_then(|n| n.checked_mul(MIB));
            let used = number(fields.next()?).and_then(|n| n.checked_mul(MIB));
            let utilization = number(fields.next()?)
                .filter(|n| *n <= 100)
                .map(|n| n as u8);
            let name: String = fields
                .next()?
                .trim()
                .trim_matches('"')
                .chars()
                .filter(|c| !c.is_control())
                .take(120)
                .collect();
            if name.is_empty() || !seen.insert(uuid.to_owned()) {
                return None;
            }
            // NVIDIA's integrated GPUs (DGX Spark's GB10, Jetson) answer the
            // other fields and report no device memory: they have no
            // framebuffer of their own. A failed read answers nothing.
            let unified = total.is_none() && (utilization.is_some() || temperature.is_some());
            Some(Device {
                index,
                uuid: uuid.into(),
                vendor: Vendor::Nvidia,
                name,
                utilization,
                used,
                total,
                temperature,
                throttle_reasons: None,
                unified,
                limits: Limits::default(),
            })
        })
        .collect();
    devices.sort_by(|a, b| a.index.cmp(&b.index).then(a.uuid.cmp(&b.uuid)));
    devices
}

fn number(field: &str) -> Option<u64> {
    field.trim().parse().ok()
}

/// A summary must cover all devices. One missing reading makes the summary
/// unavailable; the remaining measured cards are still visible individually.
pub(crate) fn peak_utilization(devices: &[Device]) -> Option<u8> {
    if devices.is_empty() {
        return None;
    }
    devices
        .iter()
        .try_fold(0, |peak, gpu| Some(peak.max(gpu.utilization?)))
}

/// What the cards draw together, in milliwatts: unknown unless every card
/// reports its power, since a part of the sum would read as all of it.
pub(crate) fn board_power_mw(devices: &[Device]) -> Option<u64> {
    if devices.is_empty() {
        return None;
    }
    devices
        .iter()
        .try_fold(0_u64, |sum, gpu| sum.checked_add(gpu.limits.power_mw?))
}

/// The cards' power caps together, in milliwatts, when every card has one.
pub(crate) fn power_cap_mw(devices: &[Device]) -> Option<u64> {
    if devices.is_empty() {
        return None;
    }
    devices
        .iter()
        .try_fold(0_u64, |sum, gpu| sum.checked_add(gpu.limits.power_cap_mw?))
}

/// The system RAM the GPUs hold, where they have no memory of their own and
/// each says how much it holds.
pub(crate) fn shared_in_use(devices: &[Device]) -> Option<u64> {
    if devices.is_empty() || !devices.iter().all(|gpu| gpu.unified) {
        return None;
    }
    devices
        .iter()
        .try_fold(0_u64, |sum, gpu| sum.checked_add(gpu.limits.ram_used?))
}

/// A card is holding its clocks down for heat (NVIDIA's thermal slowdowns):
/// the input the correlation engine judges as thermal limiting.
pub(crate) fn heat_limited(devices: &[Device]) -> bool {
    devices
        .iter()
        .any(|gpu| gpu.throttle() == Some(Throttle::Thermal))
}

/// `183/450 W`, `183 W` without a cap, `—` without a reading.
pub(crate) fn power_label(power_mw: Option<u64>, cap_mw: Option<u64>) -> String {
    let watts = |milliwatts: u64| (milliwatts + 500) / 1000;
    match (power_mw, cap_mw) {
        (Some(power), Some(cap)) => format!("{}/{} W", watts(power), watts(cap)),
        (Some(power), None) => format!("{} W", watts(power)),
        (None, _) => "—".into(),
    }
}

/// `2520/3105 MHz`, `2520 MHz` without a maximum, `—` without a reading.
pub(crate) fn clock_label(clock: Option<u32>, max: Option<u32>) -> String {
    match (clock, max) {
        (Some(clock), Some(max)) => format!("{clock}/{max} MHz"),
        (Some(clock), None) => format!("{clock} MHz"),
        (None, _) => "—".into(),
    }
}

/// `Gen4 x16`, or what is known of it; `—` with neither.
pub(crate) fn link_label(generation: Option<u8>, width: Option<u8>) -> String {
    match (generation, width) {
        (Some(generation), Some(width)) => format!("Gen{generation} x{width}"),
        (Some(generation), None) => format!("Gen{generation}"),
        (None, Some(width)) => format!("x{width}"),
        (None, None) => "—".into(),
    }
}

/// The vendors present, for panel titles: `NVIDIA`, `AMD` or `NVIDIA + AMD`.
pub(crate) fn vendors(devices: &[Device]) -> String {
    let mut names: Vec<&str> = Vec::new();
    for gpu in devices {
        if !names.contains(&gpu.vendor.label()) {
            names.push(gpu.vendor.label());
        }
    }
    names.join(" + ")
}

/// Where the counters come from, for the panel footer.
pub(crate) fn source(devices: &[Device]) -> String {
    let mut sources: Vec<&str> = Vec::new();
    for gpu in devices {
        if !sources.contains(&gpu.vendor.source()) {
            sources.push(gpu.vendor.source());
        }
    }
    sources.join(" + ")
}

/// Where the GPUs' memory comes from, which decides how memory is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Memory {
    /// The cards have memory of their own: `used` of `total`, all cards.
    Dedicated { used: u64, total: u64 },
    /// The GPU has no memory of its own: what it uses is system RAM (Apple
    /// Silicon, NVIDIA's integrated GPUs such as DGX Spark and Jetson, and
    /// AMD APUs such as Strix Halo).
    Shared,
    /// No GPU, or its readings did not arrive.
    Unknown,
}

/// Classify the cards' memory. A card with its own memory reports a total;
/// a GPU without (`Device::unified`) works in RAM. A failed read answers
/// nothing and stays unknown.
pub(crate) fn memory(devices: &[Device]) -> Memory {
    if let Some((used, total)) = memory_totals(devices).filter(|(_, total)| *total > 0) {
        return Memory::Dedicated { used, total };
    }
    if !devices.is_empty() && devices.iter().all(|gpu| gpu.unified) {
        Memory::Shared
    } else {
        Memory::Unknown
    }
}

pub(crate) fn memory_totals(devices: &[Device]) -> Option<(u64, u64)> {
    if devices.is_empty() {
        return None;
    }
    devices
        .iter()
        .try_fold((0_u64, 0_u64), |(used, total), gpu| {
            Some((used.checked_add(gpu.used?)?, total.checked_add(gpu.total?)?))
        })
}

#[cfg(test)]
#[path = "tests/gpu.rs"]
mod tests;
