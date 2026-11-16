// SPDX-License-Identifier: MIT
//! How fast a chip can read its memory at most, as its maker publishes it.
//! Decode reads every weight for each token, so this peak bounds generation
//! speed. The tables hold hardware facts only, never benchmark results, and
//! every row cites where its figure comes from.
//!
//! A chip is known by what mlxtop already reads: on a Mac the chip's name
//! from `ioreg`, its GPU core count and its RAM; elsewhere the name
//! `nvidia-smi` or amdgpu gives the card, and the card's own memory. Bins of
//! one chip differ (a 32-core M4 Max reads 410 GB/s, a 40-core one 546), so a
//! chip whose bin those readings cannot settle is unknown, never guessed. New
//! chips ship every year, and the tables need a look as they do.
use crate::domain::Sample;
use crate::gpu;

/// A chip's peak memory bandwidth.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Bandwidth {
    /// Gigabytes (10⁹ bytes) a second.
    pub(crate) gb_s: f64,
}

impl Bandwidth {
    /// As the screen shows it: `410 GB/s`.
    pub(crate) fn label(self) -> String {
        format!("{:.0} GB/s", self.gb_s)
    }
}

/// Apple silicon: the chip as `ioreg` names it, less "Apple"; the GPU core
/// counts of the bin; the RAM sizes in GiB that decide the figure, or none
/// where RAM does not; and Apple's figure in GB/s, which rounds the
/// arithmetic of bus width and memory speed.
const APPLE: &[(&str, &[u16], &[u64], f64)] = &[
    // Apple publishes no figure for M1. Secondary: LPDDR4X-4266 on a 128-bit
    // bus is 68.25 GB/s. https://en.wikipedia.org/wiki/Apple_silicon
    ("M1", &[7, 8], &[], 68.25),
    // https://support.apple.com/en-us/111902
    ("M1 Pro", &[14, 16], &[], 200.0),
    // https://support.apple.com/en-us/111900
    ("M1 Max", &[24, 32], &[], 400.0),
    ("M1 Ultra", &[48, 64], &[], 800.0),
    // https://support.apple.com/en-us/111867
    ("M2", &[8, 10], &[], 100.0),
    // https://support.apple.com/en-us/111340
    ("M2 Pro", &[16, 19], &[], 200.0),
    // https://support.apple.com/en-us/111835
    ("M2 Max", &[30, 38], &[], 400.0),
    ("M2 Ultra", &[60, 76], &[], 800.0),
    // https://support.apple.com/en-us/118551
    ("M3", &[8, 10], &[], 100.0),
    // https://support.apple.com/en-us/117736
    ("M3 Pro", &[14, 18], &[], 150.0),
    ("M3 Max", &[30], &[], 300.0),
    ("M3 Max", &[40], &[], 400.0),
    // https://support.apple.com/en-us/122211
    ("M3 Ultra", &[60, 80], &[], 819.0),
    // https://support.apple.com/en-us/122209
    ("M4", &[8, 10], &[], 120.0),
    // https://support.apple.com/en-us/121553
    ("M4 Pro", &[16, 20], &[], 273.0),
    // https://support.apple.com/en-us/122211
    ("M4 Max", &[32], &[], 410.0),
    ("M4 Max", &[40], &[], 546.0),
    // https://www.apple.com/macbook-air/specs/
    ("M5", &[8, 10], &[], 153.0),
    // https://www.apple.com/macbook-pro/specs/
    ("M5 Pro", &[16, 20], &[], 307.0),
    // https://www.apple.com/mac-studio/specs/
    ("M5 Max", &[32], &[], 460.0),
    ("M5 Max", &[40], &[], 614.0),
    // Apple gives "1.2TB/s", rounded. https://www.apple.com/mac-studio/specs/
    ("M5 Ultra", &[64, 80], &[], 1200.0),
    // RAM decides M6: 153GB/s with 16GB, 170GB/s with 24GB or 32GB.
    // https://www.apple.com/macbook-pro/specs/
    ("M6", &[12], &[16], 153.0),
    ("M6", &[12], &[24, 32], 170.0),
    // Left out: the iMac's 10-core-GPU M6, whose page gives only "up to
    // 170GB/s" and not which memory sizes reach it.
    // https://www.apple.com/imac/specs/
];

/// Cards, and GPUs that share the system's memory: the name `nvidia-smi` or
/// amdgpu gives them, less the maker's and NVIDIA's "GeForce"; the card's
/// memory sizes in GiB that decide the figure, or none where the name does;
/// and the maker's figure in GB/s.
const CARDS: &[(&str, &[u64], f64)] = &[
    // NVIDIA's tables of GeForce cards, appendices A to C.
    // https://images.nvidia.com/aem-dam/Solutions/geforce/blackwell/nvidia-rtx-blackwell-gpu-architecture.pdf
    ("RTX 3070", &[], 448.0),
    ("RTX 3070 Ti", &[], 608.0),
    ("RTX 3090", &[], 936.0),
    ("RTX 4070 Ti", &[], 504.0),
    ("RTX 4080", &[], 716.8),
    ("RTX 4090", &[], 1008.0),
    ("RTX 5070", &[], 672.0),
    ("RTX 5070 Ti", &[], 896.0),
    ("RTX 5080", &[], 960.0),
    ("RTX 5090", &[], 1792.0),
    // The 10 GB RTX 3080. A 12 GB card shares its name with a wider bus and
    // is left out.
    // https://images.nvidia.com/aem-dam/Solutions/geforce/blackwell/nvidia-rtx-blackwell-gpu-architecture.pdf
    // https://www.nvidia.com/en-us/geforce/graphics-cards/30-series/rtx-3080-3080ti/
    ("RTX 3080", &[10], 760.0),
    // Left out: the RTX 4070, sold with GDDR6X or GDDR6 under one name; the
    // two differ in bandwidth and nothing mlxtop reads tells them apart.
    // https://www.nvidia.com/en-us/geforce/graphics-cards/40-series/rtx-4070-family/

    // https://images.nvidia.com/aem-dam/Solutions/geforce/ada/nvidia-ada-gpu-architecture.pdf
    ("RTX 3080 Ti", &[], 912.0),
    ("RTX 3090 Ti", &[], 1008.0),
    // https://www.nvidia.com/content/dam/en-zz/Solutions/design-visualization/rtx-6000/proviz-print-rtx6000-datasheet-web-2504660.pdf
    ("RTX 6000 Ada Generation", &[], 960.0),
    // https://www.nvidia.com/en-us/products/workstations/professional-desktop-gpus/rtx-pro-6000/
    ("RTX PRO 6000 Blackwell Workstation Edition", &[], 1792.0),
    // The 80 GB A100 in either form; a bare "A100" could be either, and
    // stays unknown. https://www.nvidia.com/en-us/data-center/a100/
    ("A100 80GB PCIe", &[], 1935.0),
    ("A100-SXM4-80GB", &[], 2039.0),
    // H100 SXM (3.35TB/s) and H100 NVL (3.9TB/s).
    // https://www.nvidia.com/en-us/data-center/h100/
    ("H100 80GB HBM3", &[], 3350.0),
    ("H100 NVL", &[], 3900.0),
    // DGX Spark and the other GB10 machines share its memory.
    // https://www.nvidia.com/en-us/products/workstations/dgx-spark/
    ("GB10", &[], 273.0),
    // https://www.amd.com/en/products/graphics/desktops/radeon/7000-series/amd-radeon-rx-7900xtx.html
    ("Radeon RX 7900 XTX", &[], 960.0),
    // https://www.amd.com/en/products/graphics/desktops/radeon/7000-series/amd-radeon-rx-7900xt.html
    ("Radeon RX 7900 XT", &[], 800.0),
    // https://www.amd.com/en/products/graphics/desktops/radeon/7000-series/amd-radeon-rx-7900-gre.html
    ("Radeon RX 7900 GRE", &[], 576.0),
    // amdgpu takes one pci.ids name for all of Navi 31. Memory tells the XTX
    // (24 GB) and XT (20 GB) apart; 16 GB is a GRE or a 7900M, left out.
    // https://pci-ids.ucw.cz/read/PC/1002/744c
    ("Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M", &[24], 960.0),
    ("Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M", &[20], 800.0),
    // https://www.amd.com/en/products/graphics/desktops/radeon/9000-series/amd-radeon-rx-9070xt.html
    ("Radeon RX 9070 XT", &[], 640.0),
    // https://www.amd.com/en/products/graphics/desktops/radeon/9000-series/amd-radeon-rx-9070.html
    ("Radeon RX 9070", &[], 640.0),
    // https://www.amd.com/en/products/graphics/desktops/radeon/9000-series/amd-radeon-rx-9070-gre.html
    ("Radeon RX 9070 GRE", &[], 432.0),
    // One pci.ids name for Navi 48: the RX 9070 and 9070 XT have 16 GB, the
    // GRE 12 GB. https://pci-ids.ucw.cz/read/PC/1002/7550
    ("Radeon RX 9070/9070 XT/9070 GRE", &[16], 640.0),
    ("Radeon RX 9070/9070 XT/9070 GRE", &[12], 432.0),
    // Strix Halo, from AMD's bus width and memory speed: 256-bit LPDDR5x-8000
    // behind the Radeon 8060S and 8050S, 128-bit behind the Ryzen AI Max PRO
    // 380's 8040S. amdgpu's pci.ids name for the chip, "Radeon Graphics /
    // Radeon 8050S Graphics / Radeon 8060S Graphics", does not say which, and
    // stays unknown. https://pci-ids.ucw.cz/read/PC/1002/1586
    // https://www.amd.com/en/products/processors/laptop/ryzen/ai-300-series/amd-ryzen-ai-max-plus-395.html
    // https://www.amd.com/en/products/processors/laptop/ryzen/ai-300-series/amd-ryzen-ai-max-390.html
    // https://www.amd.com/en/products/processors/laptop/ryzen-pro/ai-max-pro-300-series/amd-ryzen-ai-max-pro-380.html
    ("Radeon 8060S Graphics", &[], 256.0),
    ("Radeon 8050S Graphics", &[], 256.0),
    ("Radeon 8040S Graphics", &[], 128.0),
];

/// The chip's name as the tables key it: the maker's name and NVIDIA's
/// GeForce brand left off, spaces collapsed.
fn key(name: &str) -> String {
    name.split_whitespace()
        .skip_while(|word| {
            ["Apple", "NVIDIA", "AMD", "GeForce"]
                .iter()
                .any(|maker| maker.eq_ignore_ascii_case(word))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The peak bandwidth of the chip `name` (as `ioreg`, `nvidia-smi` or amdgpu
/// give it), with its GPU core count and memory in bytes where they are
/// known. A Mac chip needs its core count; a row that lists memory sizes
/// needs the memory. Unknown when no row holds for these readings.
pub(crate) fn lookup(name: &str, gpu_cores: Option<u16>, memory: Option<u64>) -> Option<Bandwidth> {
    let name = key(name);
    // RAM and a card's memory are whole GiB, give or take what the driver keeps.
    let gib = memory
        .filter(|bytes| *bytes > 0)
        .map(|bytes| bytes.saturating_add(1 << 29) >> 30);
    let fits = |sizes: &[u64]| sizes.is_empty() || gib.is_some_and(|gib| sizes.contains(&gib));
    APPLE
        .iter()
        .find(|(chip, cores, ram, _)| {
            chip.eq_ignore_ascii_case(&name)
                && gpu_cores.is_some_and(|count| cores.contains(&count))
                && fits(ram)
        })
        .map(|row| row.3)
        .or_else(|| {
            CARDS
                .iter()
                .find(|(card, sizes, _)| card.eq_ignore_ascii_case(&name) && fits(sizes))
                .map(|row| row.2)
        })
        .map(|gb_s| Bandwidth { gb_s })
}

/// A card as `nvidia-smi` or amdgpu report it, with its own memory.
pub(crate) fn card(gpu: &gpu::Device) -> Option<Bandwidth> {
    lookup(&gpu.name, None, gpu.total)
}

/// The chip the host footer names: the one card `nvidia-smi` or amdgpu
/// lists, else the Mac's chip. Several cards have no single figure.
pub(crate) fn of_sample(sample: &Sample) -> Option<Bandwidth> {
    if sample.has_discrete_gpus() {
        return match sample.gpus.as_slice() {
            [gpu] => card(gpu),
            _ => None,
        };
    }
    lookup(
        sample.metal.device_name.as_deref()?,
        sample.metal.gpu_cores,
        Some(sample.total_memory),
    )
}

#[cfg(test)]
#[path = "tests/chip_facts.rs"]
mod tests;
