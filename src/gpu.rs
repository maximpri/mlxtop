// SPDX-License-Identifier: MIT
//! Per-device NVIDIA counters. Device memory is separate from Apple Metal accounting.
//! Collected on Linux and Windows, where `nvidia-smi` ships with the driver.

use crate::domain::MIB;
use std::collections::HashSet;

use crate::host::Host;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Device {
    pub index: u32,
    pub uuid: String,
    pub name: String,
    pub utilization: Option<u8>,
    pub used: Option<u64>,
    pub total: Option<u64>,
    pub temperature: Option<u64>,
    /// Active clock-limit reasons as the NVML bitmask; `None` when the driver
    /// does not report them.
    pub throttle_reasons: Option<u64>,
}

/// Why the driver is holding clocks below their maximum, most severe first.
/// Idle, application-clock and sync-boost reasons are normal and excluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

impl Device {
    pub fn memory_percent(&self) -> Option<u16> {
        let (used, total) = self.used.zip(self.total)?;
        (total > 0).then(|| ((u128::from(used) * 100 / u128::from(total)).min(100)) as u16)
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

    fn unavailable(&self) -> Self {
        Self {
            utilization: None,
            used: None,
            total: None,
            temperature: None,
            throttle_reasons: None,
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

/// Query all physical devices in one invocation. UUIDs distinguish identical
/// models and survive driver enumeration changes; indices remain familiar labels.
pub(crate) fn collect(host: &dyn Host, previous: &[Device]) -> Vec<Device> {
    if !cfg!(any(target_os = "linux", target_os = "windows")) {
        return Vec::new();
    }
    let output = host.command(
        "nvidia-smi",
        &[
            "--query-gpu=index,uuid,name,utilization.gpu,memory.used,memory.total,temperature.gpu",
            "--format=csv,noheader,nounits",
        ],
    );
    let mut devices = readings(output.as_deref(), previous);
    if devices.iter().any(|gpu| gpu.utilization.is_some()) {
        let reasons = THROTTLE_FIELDS
            .iter()
            .find_map(|fields| host.command("nvidia-smi", &[fields, "--format=csv,noheader"]));
        if let Some(text) = reasons {
            apply_throttle_reasons(&mut devices, &text);
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
            Some(Device {
                index,
                uuid: uuid.into(),
                name,
                utilization,
                used,
                total,
                temperature,
                throttle_reasons: None,
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
