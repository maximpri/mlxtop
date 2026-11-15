// SPDX-License-Identifier: MIT
//! Per-device AMD counters from the amdgpu kernel driver's sysfs files.
//! Collected on Linux only, without extra tools: the driver publishes VRAM,
//! GTT, utilization, memory activity, power, clocks and temperatures under
//! `/sys/class/drm/card*/device`.

use crate::gpu::{Device, Limits, Vendor};
use crate::host::Host;
use std::path::{Path, PathBuf};

const DRM: &str = "/sys/class/drm";
const AMD_VENDOR: &str = "0x1002";
/// A dedicated card has at least this much VRAM. Less is an integrated GPU's
/// BIOS carve-out of system RAM.
const MIN_VRAM: u64 = 1 << 30;
/// pci.ids locations on Debian/Ubuntu/Fedora and Arch, tried in order.
const PCI_IDS: [&str; 2] = ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"];

/// Every amdgpu card with readable VRAM, in DRM card order. Names are looked
/// up once per card and carried over from `previous`.
pub(crate) fn collect(host: &dyn Host, previous: &[Device]) -> Vec<Device> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    cards(host, previous)
}

/// `collect` on any host. An APU (Strix Halo and the like) is marked
/// `unified`: its VRAM is a carve-out of system RAM, and a model on it lives
/// mostly in GTT, the system RAM the GPU maps. Its memory is then the RAM it
/// holds, carve-out and GTT together, never a near-empty VRAM meter.
pub(crate) fn cards(host: &dyn Host, previous: &[Device]) -> Vec<Device> {
    let mut cards: Vec<PathBuf> = host
        .read_dir(Path::new(DRM))
        .into_iter()
        .filter(|path| card_number(path).is_some())
        .collect();
    cards.sort_by_key(|path| card_number(path));
    let mut devices = Vec::new();
    for card in cards {
        let dev = card.join("device");
        if read(host, &dev.join("vendor")).as_deref() != Some(AMD_VENDOR) {
            continue;
        }
        let Some(uuid) = identity(host, &dev) else {
            continue;
        };
        let total = number(host, &dev.join("mem_info_vram_total"));
        let used = number(host, &dev.join("mem_info_vram_used"))
            .filter(|used| total.is_none_or(|total| *used <= total));
        let awake = awake(host, &dev);
        // Every sensor and the GTT counters are read only while the card is
        // awake: see `awake`. While a card sleeps its VRAM is evicted to GTT,
        // which would read as the card's own work.
        let hwmon = if awake {
            host.read_dir(&dev.join("hwmon"))
        } else {
            Vec::new()
        };
        // amdgpu gives the northbridge voltage sensor to APUs alone, so a
        // large carve-out is recognised as well as the usual small one.
        let unified = total.is_some_and(|total| total < MIN_VRAM)
            || hwmon
                .iter()
                .any(|dir| read(host, &dir.join("in1_label")).as_deref() == Some("vddnb"));
        let name = previous
            .iter()
            .find(|gpu| gpu.uuid == uuid)
            .map(|gpu| gpu.name.clone())
            .unwrap_or_else(|| device_name(host, &dev));
        let mut limits = if awake {
            readings(host, &dev, &hwmon)
        } else {
            Limits::default()
        };
        let gtt = |file: &str| awake.then(|| number(host, &dev.join(file))).flatten();
        let (gtt_used, gtt_total) = (gtt("mem_info_gtt_used"), gtt("mem_info_gtt_total"));
        if unified {
            // The carve-out is RAM too: what the GPU holds of RAM is both.
            limits.ram_used = used
                .zip(gtt_used)
                .map(|(vram, gtt)| vram.saturating_add(gtt));
            limits.ram_limit = total
                .zip(gtt_total)
                .map(|(vram, gtt)| vram.saturating_add(gtt));
        } else {
            limits.ram_used = gtt_used;
            limits.ram_limit = gtt_total;
        }
        devices.push(Device {
            index: devices.len() as u32,
            uuid,
            vendor: Vendor::Amd,
            name,
            utilization: awake
                .then(|| number(host, &dev.join("gpu_busy_percent")))
                .flatten()
                .filter(|n| *n <= 100)
                .map(|n| n as u8),
            used: used.filter(|_| !unified),
            total: total.filter(|_| !unified),
            temperature: awake.then(|| temperature(host, &hwmon)).flatten(),
            throttle_reasons: None,
            unified,
            limits,
        });
    }
    devices
}

/// Memory activity, power against its cap, and the graphics clock against
/// its top level. APUs report no memory activity and most no power cap.
fn readings(host: &dyn Host, dev: &Path, hwmon: &[PathBuf]) -> Limits {
    let sensor = |file: &str| hwmon.iter().find_map(|dir| number(host, &dir.join(file)));
    // Microwatts; newer cards report the instant reading only.
    let milliwatts = |microwatts: u64| (microwatts > 0).then(|| (microwatts + 500) / 1000);
    Limits {
        power_mw: sensor("power1_average")
            .or_else(|| sensor("power1_input"))
            .and_then(milliwatts),
        power_cap_mw: sensor("power1_cap").and_then(milliwatts),
        // Hertz.
        clock_mhz: sensor("freq1_input")
            .map(|hertz| (hertz + 500_000) / 1_000_000)
            .and_then(|mhz| u32::try_from(mhz).ok())
            .filter(|mhz| *mhz > 0),
        clock_max_mhz: read(host, &dev.join("pp_dpm_sclk"))
            .as_deref()
            .and_then(top_level_mhz),
        memory_busy: number(host, &dev.join("mem_busy_percent"))
            .filter(|n| *n <= 100)
            .map(|n| n as u8),
        ..Limits::default()
    }
}

/// The highest clock level in a `pp_dpm_*` table (`0: 500Mhz`, `1: 2482Mhz
/// *`, a deep-sleep `S: 19Mhz`), in MHz.
pub(crate) fn top_level_mhz(table: &str) -> Option<u32> {
    table
        .lines()
        .filter_map(|line| {
            let (_, level) = line.split_once(':')?;
            let digits: String = level
                .trim()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse::<u32>().ok().filter(|mhz| *mhz > 0)
        })
        .max()
}

/// Whether the card is powered up for counter reads. Before Linux 6.13 the
/// busy and temperature files call `pm_runtime_get_sync`, which resumes a
/// runtime-suspended card, so polling would keep a laptop's dGPU awake.
/// `unsupported` means runtime PM is off and the card stays powered. VRAM
/// counters never wake the card and are read either way.
fn awake(host: &dyn Host, dev: &Path) -> bool {
    read(host, &dev.join("power/runtime_status"))
        .is_none_or(|status| status == "active" || status == "unsupported")
}

fn card_number(path: &Path) -> Option<u32> {
    path.file_name()?
        .to_str()?
        .strip_prefix("card")?
        .parse()
        .ok()
}

fn read(host: &dyn Host, path: &Path) -> Option<String> {
    let text = host.read_file(path)?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn number(host: &dyn Host, path: &Path) -> Option<u64> {
    read(host, path)?.parse().ok()
}

/// `AMD-` plus the driver's stable unique id, or the PCI slot on cards that
/// lack one. Either survives DRM renumbering between boots.
fn identity(host: &dyn Host, dev: &Path) -> Option<String> {
    let id = read(host, &dev.join("unique_id")).or_else(|| {
        read(host, &dev.join("uevent"))?
            .lines()
            .find_map(|line| line.strip_prefix("PCI_SLOT_NAME="))
            .map(str::to_owned)
    })?;
    if id.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    Some(format!("AMD-{id}"))
}

/// The marketing name from the driver when present, else from pci.ids
/// (`Navi 48 [Radeon RX 9070/9070 XT/9070 GRE]` becomes `AMD Radeon RX
/// 9070/9070 XT/9070 GRE`), else the PCI device id.
fn device_name(host: &dyn Host, dev: &Path) -> String {
    let clean =
        |name: &str| -> String { name.chars().filter(|c| !c.is_control()).take(120).collect() };
    if let Some(name) = read(host, &dev.join("product_name")) {
        return clean(&name);
    }
    let device = read(host, &dev.join("device"));
    let id = device
        .as_deref()
        .and_then(|id| id.strip_prefix("0x"))
        .map(str::to_ascii_lowercase);
    if let Some(id) = &id {
        let found = PCI_IDS
            .iter()
            .find_map(|path| host.read_file(Path::new(path)))
            .and_then(|ids| pci_ids_name(&ids, id));
        if let Some(name) = found {
            return clean(&format!("AMD {name}"));
        }
    }
    format!("AMD GPU 1002:{}", id.unwrap_or_else(|| "????".into()))
}

/// The device line under vendor 1002 in pci.ids, preferring the bracketed
/// product name over the chip codename.
pub(crate) fn pci_ids_name(ids: &str, device: &str) -> Option<String> {
    let mut in_amd = false;
    for line in ids.lines() {
        // Comments sit inside vendor blocks and must not end them.
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            in_amd = line.starts_with("1002 ");
            continue;
        }
        if !in_amd || line.starts_with("\t\t") {
            continue;
        }
        let Some((id, name)) = line[1..].split_once(char::is_whitespace) else {
            continue;
        };
        if id.eq_ignore_ascii_case(device) {
            let name = name.trim();
            let product = name
                .split_once('[')
                .and_then(|(_, rest)| rest.rsplit_once(']'))
                .map(|(inner, _)| inner.trim())
                .filter(|inner| !inner.is_empty());
            return Some(product.unwrap_or(name).to_owned());
        }
    }
    None
}

/// The edge sensor, the closest match to the core temperature NVIDIA
/// reports; the first hwmon reading when no sensor is labelled.
fn temperature(host: &dyn Host, hwmon: &[PathBuf]) -> Option<u64> {
    let mut first = None;
    for hwmon in hwmon {
        for sensor in 1..=3 {
            let Some(millidegrees) = number(host, &hwmon.join(format!("temp{sensor}_input")))
            else {
                continue;
            };
            let celsius = millidegrees / 1000;
            let label = read(host, &hwmon.join(format!("temp{sensor}_label")));
            if label.as_deref() == Some("edge") {
                return Some(celsius);
            }
            first.get_or_insert(celsius);
        }
    }
    first
}

#[cfg(test)]
#[path = "tests/gpu_amd.rs"]
mod tests;
