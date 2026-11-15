// SPDX-License-Identifier: MIT
use super::*;
use crate::test_support::FakeHost;

/// Excerpt of pci.ids around an RX 9070, with the comment lines the real
/// file has inside the AMD block.
const PCI_IDS_EXCERPT: &str = "\
# List of PCI ID's
1000  Broadcom / LSI
\t0001  53c810
1002  Advanced Micro Devices, Inc. [AMD/ATI]
# Used in the Steam Deck OLED
\t1435  Aerith
\t7550  Navi 48 [Radeon RX 9070/9070 XT/9070 GRE]
\t\t1da2 e490  Navi 48 XTX [Sapphire Pulse Radeon RX 9070 XT]
\t7551  Navi 48 XL
1003  ULSI Systems
\t0201  US201
";

/// One RX 9070 as the amdgpu driver presents it under sysfs on Linux,
/// readings recorded from a real card.
fn rx9070(host: FakeHost, card: &str, unique_id: &str) -> FakeHost {
    let dev = format!("/sys/class/drm/{card}/device");
    let hwmon = format!("{dev}/hwmon/hwmon3");
    host.file(&format!("{dev}/vendor"), "0x1002\n")
        .file(&format!("{dev}/device"), "0x7550\n")
        .file(&format!("{dev}/unique_id"), unique_id)
        .file(&format!("{dev}/mem_info_vram_total"), "17095983104\n")
        .file(&format!("{dev}/mem_info_vram_used"), "2323398656\n")
        .file(&format!("{dev}/gpu_busy_percent"), "5\n")
        .dir(&format!("{dev}/hwmon"), &[&hwmon])
        .file(&format!("{hwmon}/temp1_label"), "edge\n")
        .file(&format!("{hwmon}/temp1_input"), "43000\n")
        .file(&format!("{hwmon}/temp2_label"), "junction\n")
        .file(&format!("{hwmon}/temp2_input"), "47000\n")
        .file("/usr/share/hwdata/pci.ids", PCI_IDS_EXCERPT)
}

#[test]
fn pci_ids_prefers_the_product_name_and_skips_comments() {
    assert_eq!(
        pci_ids_name(PCI_IDS_EXCERPT, "7550").as_deref(),
        Some("Radeon RX 9070/9070 XT/9070 GRE")
    );
    assert_eq!(
        pci_ids_name(PCI_IDS_EXCERPT, "7551").as_deref(),
        Some("Navi 48 XL")
    );
    assert_eq!(
        pci_ids_name(PCI_IDS_EXCERPT, "1435").as_deref(),
        Some("Aerith")
    );
    // Device ids of other vendors and subsystem rows never match.
    assert_eq!(pci_ids_name(PCI_IDS_EXCERPT, "0001"), None);
    assert_eq!(pci_ids_name(PCI_IDS_EXCERPT, "1da2"), None);
    assert_eq!(pci_ids_name("", "7550"), None);
}

#[cfg(target_os = "linux")]
#[test]
fn reads_a_discrete_card_from_sysfs() {
    let host = rx9070(
        FakeHost::default().dir(
            "/sys/class/drm",
            &[
                "/sys/class/drm/card1",
                "/sys/class/drm/card1-DP-1",
                "/sys/class/drm/renderD128",
                "/sys/class/drm/version",
            ],
        ),
        "card1",
        "5b7c23d1d3025b6c\n",
    );
    let cards = collect(&host, &[]);
    assert_eq!(cards.len(), 1);
    let card = &cards[0];
    assert_eq!(card.index, 0);
    assert_eq!(card.uuid, "AMD-5b7c23d1d3025b6c");
    assert_eq!(card.vendor, Vendor::Amd);
    assert_eq!(card.name, "AMD Radeon RX 9070/9070 XT/9070 GRE");
    assert_eq!(card.utilization, Some(5));
    assert_eq!(card.used, Some(2323398656));
    assert_eq!(card.total, Some(17095983104));
    assert_eq!(card.memory_percent(), Some(13));
    // The edge sensor, not the hotter junction.
    assert_eq!(card.temperature, Some(43));
    assert_eq!(card.throttle_reasons, None);
}

#[cfg(target_os = "linux")]
#[test]
fn a_sleeping_card_is_listed_without_waking_it() {
    let dev = "/sys/class/drm/card1/device";
    let host = |status: &str| {
        rx9070(
            FakeHost::default().dir("/sys/class/drm", &["/sys/class/drm/card1"]),
            "card1",
            "5b7c23d1d3025b6c\n",
        )
        .file(
            &format!("{dev}/power/runtime_status"),
            &format!("{status}\n"),
        )
    };
    for status in ["suspended", "suspending", "resuming", "error"] {
        let host = host(status);
        let card = &collect(&host, &[])[0];
        assert_eq!(card.uuid, "AMD-5b7c23d1d3025b6c");
        // VRAM counters never resume the card; busy and temperature reads
        // do on kernels before 6.13, so they are not touched at all.
        assert_eq!(card.used, Some(2323398656));
        assert_eq!(card.utilization, None);
        assert_eq!(card.temperature, None);
        assert!(
            !host
                .calls()
                .iter()
                .any(|call| call.contains("gpu_busy_percent") || call.contains("hwmon")),
            "{status}"
        );
    }
    // Runtime PM switched off keeps the card powered, so it is read.
    for status in ["active", "unsupported"] {
        let host = host(status);
        let card = &collect(&host, &[])[0];
        assert_eq!(card.utilization, Some(5));
        assert_eq!(card.temperature, Some(43));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn skips_other_vendors_and_lists_a_small_carve_out_as_ram() {
    let host = FakeHost::default()
        .dir(
            "/sys/class/drm",
            &["/sys/class/drm/card0", "/sys/class/drm/card2"],
        )
        // An Intel iGPU.
        .file("/sys/class/drm/card0/device/vendor", "0x8086\n")
        // An AMD APU reporting its 512 MiB BIOS carve-out as VRAM.
        .file("/sys/class/drm/card2/device/vendor", "0x1002\n")
        .file("/sys/class/drm/card2/device/unique_id", "abc\n")
        .file(
            "/sys/class/drm/card2/device/mem_info_vram_total",
            "536870912\n",
        );
    let cards = collect(&host, &[]);
    assert_eq!(cards.len(), 1);
    // The carve-out is RAM, never a VRAM meter that reads full.
    assert!(cards[0].unified);
    assert_eq!((cards[0].used, cards[0].total), (None, None));
    assert_eq!(cards[0].memory_percent(), None);
}

/// pci.ids' line for Strix Halo's GPU.
const STRIX_HALO_IDS: &str = "\
1002  Advanced Micro Devices, Inc. [AMD/ATI]
\t1586  Strix Halo [Radeon Graphics / Radeon 8050S / 8060S Graphics]
";

/// A Strix Halo APU (Ryzen AI Max+ 395, Radeon 8060S) as amdgpu presents it
/// with AMD's recommended setup: a 512 MiB carve-out and a large GTT, which
/// is where the model lives. APUs report no memory activity and, on this
/// generation, no power cap.
fn strix_halo(carve_out: u64, carve_out_used: u64) -> FakeHost {
    let dev = "/sys/class/drm/card0/device";
    let hwmon = format!("{dev}/hwmon/hwmon5");
    FakeHost::default()
        .dir("/sys/class/drm", &["/sys/class/drm/card0"])
        .file(&format!("{dev}/vendor"), "0x1002\n")
        .file(&format!("{dev}/device"), "0x1586\n")
        .file(
            &format!("{dev}/uevent"),
            "DRIVER=amdgpu\nPCI_SLOT_NAME=0000:c5:00.0\n",
        )
        .file(&format!("{dev}/power/runtime_status"), "active\n")
        .file(
            &format!("{dev}/mem_info_vram_total"),
            &format!("{carve_out}\n"),
        )
        .file(
            &format!("{dev}/mem_info_vram_used"),
            &format!("{carve_out_used}\n"),
        )
        .file(&format!("{dev}/mem_info_gtt_total"), "120259084288\n")
        .file(&format!("{dev}/mem_info_gtt_used"), "62277025792\n")
        .file(&format!("{dev}/gpu_busy_percent"), "97\n")
        .file(
            &format!("{dev}/pp_dpm_sclk"),
            "0: 600Mhz\n1: 2826Mhz *\n2: 2900Mhz\n",
        )
        .dir(&format!("{dev}/hwmon"), &[&hwmon])
        .file(&format!("{hwmon}/in1_label"), "vddnb\n")
        .file(&format!("{hwmon}/temp1_label"), "edge\n")
        .file(&format!("{hwmon}/temp1_input"), "64000\n")
        .file(&format!("{hwmon}/power1_input"), "86000000\n")
        .file(&format!("{hwmon}/freq1_input"), "2826000000\n")
        .file("/usr/share/hwdata/pci.ids", STRIX_HALO_IDS)
}

const GIB: u64 = 1 << 30;

#[test]
fn a_strix_halo_apu_holds_ram_not_vram() {
    let host = strix_halo(512 * 1024 * 1024, 384 * 1024 * 1024);
    let cards = cards(&host, &[]);
    assert_eq!(cards.len(), 1);
    let apu = &cards[0];
    assert_eq!(apu.uuid, "AMD-0000:c5:00.0");
    assert_eq!(
        apu.name,
        "AMD Radeon Graphics / Radeon 8050S / 8060S Graphics"
    );
    assert!(apu.unified);
    // No VRAM of its own: no near-empty VRAM meter, no VRAM FULL finding.
    assert_eq!((apu.used, apu.total), (None, None));
    assert_eq!(apu.memory_percent(), None);
    // What it holds of RAM: the carve-out and GTT, of what it may hold.
    assert_eq!(
        apu.limits.ram_used,
        Some(384 * 1024 * 1024 + 62_277_025_792)
    );
    assert_eq!(
        apu.limits.ram_limit,
        Some(512 * 1024 * 1024 + 120_259_084_288)
    );
    assert_eq!(apu.utilization, Some(97));
    assert_eq!(apu.temperature, Some(64));
    assert_eq!(apu.limits.power_mw, Some(86_000));
    assert_eq!(apu.limits.power_cap_mw, None);
    assert_eq!(
        (apu.limits.clock_mhz, apu.limits.clock_max_mhz),
        (Some(2826), Some(2900))
    );
    assert_eq!(apu.limits.memory_busy, None);
    assert_eq!(crate::gpu::memory(&cards), crate::gpu::Memory::Shared);
    assert_eq!(crate::gpu::shared_in_use(&cards), apu.limits.ram_used);
    assert_eq!(crate::gpu::merge(Vec::new(), cards.clone()), cards);
    assert_eq!(crate::gpu_dashboard::memory_label(apu), "58.4/112.5 GiB");
    // A carve-out set large in the BIOS is still RAM: amdgpu gives the
    // northbridge voltage sensor to APUs alone.
    let large = cards_with(strix_halo(64 * GIB, 60 * GIB));
    assert!(large[0].unified);
    assert_eq!(large[0].total, None);
    assert_eq!(large[0].limits.ram_used, Some(60 * GIB + 62_277_025_792));
}

fn cards_with(host: FakeHost) -> Vec<Device> {
    cards(&host, &[])
}

#[test]
fn a_strix_halo_draws_one_pool_with_the_gpu_inside_ram() {
    use crate::domain::{Sample, Tone};
    let devices = cards_with(strix_halo(512 * 1024 * 1024, 384 * 1024 * 1024));
    // 128 GiB, as Linux counts it: GTT pages are neither apps' memory nor
    // available, so they are part of what the panel draws as wired.
    let total = 128 * GIB - 512 * 1024 * 1024;
    let sample = Sample {
        total_memory: total,
        vm_available: true,
        wired: 62 * GIB,
        anonymous: 6 * GIB,
        file_backed: 20 * GIB,
        pressure: "GREEN".into(),
        pressure_tone: Tone::Green,
        gpu_in_use: crate::gpu::shared_in_use(&devices),
        gpus: devices.clone(),
        ..Sample::default()
    };
    let render = |memory: crate::gpu::Memory| {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 8)).unwrap();
        terminal
            .draw(|frame| {
                crate::memory_headroom::draw_for(frame, frame.area(), &sample, memory, |_| {
                    Vec::new()
                });
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let screen = render(crate::gpu::memory(&devices));
    assert!(screen.contains("RAM"), "{screen}");
    assert!(
        !screen.contains("VRAM"),
        "one pool, no VRAM meter:\n{screen}"
    );
    // The GPU's part is drawn inside RAM, with its size.
    assert!(screen.contains("GPU 58.4"), "{screen}");
    // What the same APU drew as a card of its own: a near-empty VRAM meter.
    let before = render(crate::gpu::Memory::Dedicated {
        used: 384 * 1024 * 1024,
        total: 512 * 1024 * 1024,
    });
    assert!(before.contains("VRAM"), "{before}");
}

#[test]
fn a_card_reads_memory_activity_power_clocks_and_gtt() {
    let dev = "/sys/class/drm/card1/device";
    let hwmon = format!("{dev}/hwmon/hwmon3");
    let host = rx9070(
        FakeHost::default().dir("/sys/class/drm", &["/sys/class/drm/card1"]),
        "card1",
        "5b7c23d1d3025b6c\n",
    )
    .file(&format!("{dev}/mem_busy_percent"), "42\n")
    .file(
        &format!("{dev}/pp_dpm_sclk"),
        "0: 500Mhz\n1: 2450Mhz *\n2: 2970Mhz\n",
    )
    .file(&format!("{dev}/mem_info_gtt_used"), "805306368\n")
    .file(&format!("{dev}/mem_info_gtt_total"), "35433480192\n")
    .file(&format!("{hwmon}/power1_average"), "183000000\n")
    .file(&format!("{hwmon}/power1_input"), "190000000\n")
    .file(&format!("{hwmon}/power1_cap"), "220000000\n")
    .file(&format!("{hwmon}/freq1_input"), "2450000000\n");
    let card = &cards(&host, &[])[0];
    assert!(!card.unified);
    assert_eq!(card.total, Some(17095983104));
    let limits = &card.limits;
    assert_eq!(limits.memory_busy, Some(42));
    // The averaged reading wins over the instant one.
    assert_eq!(limits.power_mw, Some(183_000));
    assert_eq!(limits.power_cap_mw, Some(220_000));
    assert_eq!(
        (limits.clock_mhz, limits.clock_max_mhz),
        (Some(2450), Some(2970))
    );
    // A card with VRAM of its own: GTT is the system RAM it holds besides.
    assert_eq!(limits.ram_used, Some(805306368));
    assert_eq!(limits.ram_limit, Some(35433480192));
    assert_eq!(limits.next_read, None, "sysfs is read every poll");
    // Newer cards have only the instant reading; nonsense stays unknown.
    let instant = rx9070(
        FakeHost::default().dir("/sys/class/drm", &["/sys/class/drm/card1"]),
        "card1",
        "5b7c23d1d3025b6c\n",
    )
    .file(&format!("{hwmon}/power1_input"), "190000000\n")
    .file(&format!("{hwmon}/power1_cap"), "0\n")
    .file(&format!("{dev}/mem_busy_percent"), "180\n")
    .file(&format!("{hwmon}/freq1_input"), "0\n");
    let limits = &cards(&instant, &[])[0].limits;
    assert_eq!(limits.power_mw, Some(190_000));
    assert_eq!(limits.power_cap_mw, None);
    assert_eq!(limits.memory_busy, None);
    assert_eq!(limits.clock_mhz, None);
    assert_eq!(limits.clock_max_mhz, None);
}

#[test]
fn a_sleeping_card_is_not_woken_for_any_reading() {
    let dev = "/sys/class/drm/card1/device";
    let hwmon = format!("{dev}/hwmon/hwmon3");
    for status in ["suspended", "suspending", "resuming", "error"] {
        let host = rx9070(
            FakeHost::default().dir("/sys/class/drm", &["/sys/class/drm/card1"]),
            "card1",
            "5b7c23d1d3025b6c\n",
        )
        .file(
            &format!("{dev}/power/runtime_status"),
            &format!("{status}\n"),
        )
        .file(&format!("{dev}/mem_busy_percent"), "42\n")
        .file(&format!("{dev}/pp_dpm_sclk"), "0: 500Mhz\n1: 2970Mhz *\n")
        .file(&format!("{dev}/mem_info_gtt_used"), "805306368\n")
        .file(&format!("{dev}/mem_info_gtt_total"), "35433480192\n")
        .file(&format!("{hwmon}/power1_average"), "183000000\n")
        .file(&format!("{hwmon}/power1_cap"), "220000000\n")
        .file(&format!("{hwmon}/freq1_input"), "2450000000\n")
        .file(&format!("{hwmon}/in1_label"), "vddnb\n");
        let card = &cards(&host, &[])[0];
        // Listed with its VRAM, which never wakes it; nothing else known.
        assert_eq!(card.used, Some(2323398656), "{status}");
        assert!(!card.unified, "{status}");
        assert_eq!(card.limits, crate::gpu::Limits::default(), "{status}");
        let calls = host.calls();
        for file in [
            "gpu_busy_percent",
            "mem_busy_percent",
            "pp_dpm_sclk",
            "mem_info_gtt",
            "hwmon",
        ] {
            assert!(
                !calls.iter().any(|call| call.contains(file)),
                "{status} read {file}: {calls:?}"
            );
        }
    }
}

#[test]
fn the_top_clock_level_is_the_highest_one_listed() {
    assert_eq!(
        top_level_mhz("S: 19Mhz *\n0: 615Mhz\n1: 800Mhz\n2: 888Mhz\n3: 1000Mhz\n"),
        Some(1000)
    );
    assert_eq!(
        top_level_mhz("0: 500Mhz\n1: 2482Mhz *\n2: 2526Mhz\n"),
        Some(2526)
    );
    assert_eq!(top_level_mhz("1: 2482MHz *"), Some(2482));
    assert_eq!(top_level_mhz(""), None);
    assert_eq!(top_level_mhz("garbage\n0: Mhz\n1: 0Mhz\n"), None);
}

#[cfg(target_os = "linux")]
#[test]
fn orders_cards_numerically_and_reuses_known_names() {
    let host = rx9070(
        rx9070(
            FakeHost::default().dir(
                "/sys/class/drm",
                &["/sys/class/drm/card10", "/sys/class/drm/card2"],
            ),
            "card10",
            "second\n",
        ),
        "card2",
        "5b7c23d1d3025b6c\n",
    );
    let cards = collect(&host, &[]);
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0].uuid, "AMD-5b7c23d1d3025b6c");
    assert_eq!(cards[1].uuid, "AMD-second");
    assert_eq!(cards[1].index, 1);

    // A known card keeps its name without another pci.ids lookup.
    let mut previous = cards.clone();
    previous[0].name = "Remembered".into();
    let again = collect(&host, &previous);
    assert_eq!(again[0].name, "Remembered");
}

#[cfg(target_os = "linux")]
#[test]
fn falls_back_for_identity_name_and_temperature() {
    let dev = "/sys/class/drm/card0/device";
    let hwmon = "/sys/class/drm/card0/device/hwmon/hwmon0";
    let host = FakeHost::default()
        .dir("/sys/class/drm", &["/sys/class/drm/card0"])
        .file(&format!("{dev}/vendor"), "0x1002\n")
        .file(&format!("{dev}/device"), "0x74A1\n")
        // No unique_id: the PCI slot identifies the card.
        .file(
            &format!("{dev}/uevent"),
            "DRIVER=amdgpu\nPCI_SLOT_NAME=0000:03:00.0\n",
        )
        .file(&format!("{dev}/mem_info_vram_total"), "205822672896\n")
        // Used above total is a bad read, not a fact.
        .file(&format!("{dev}/mem_info_vram_used"), "305822672896\n")
        .file(&format!("{dev}/gpu_busy_percent"), "250\n")
        .dir(&format!("{dev}/hwmon"), &[hwmon])
        .file(&format!("{hwmon}/temp1_input"), "61000\n");
    let card = &collect(&host, &[])[0];
    assert_eq!(card.uuid, "AMD-0000:03:00.0");
    // No pci.ids on this host.
    assert_eq!(card.name, "AMD GPU 1002:74a1");
    assert_eq!(card.used, None);
    assert_eq!(card.utilization, None);
    assert_eq!(card.temperature, Some(61));

    let named = host
        .clone()
        .file(&format!("{dev}/product_name"), "AMD Instinct MI300X\n");
    assert_eq!(collect(&named, &[])[0].name, "AMD Instinct MI300X");
}

#[cfg(target_os = "linux")]
#[test]
fn a_card_without_any_identity_is_left_out() {
    let host = FakeHost::default()
        .dir("/sys/class/drm", &["/sys/class/drm/card0"])
        .file("/sys/class/drm/card0/device/vendor", "0x1002\n")
        .file("/sys/class/drm/card0/device/unique_id", "bad id\n");
    assert!(collect(&host, &[]).is_empty());
}

#[cfg(not(target_os = "linux"))]
#[test]
fn amd_collection_is_linux_only() {
    let host = rx9070(
        FakeHost::default().dir("/sys/class/drm", &["/sys/class/drm/card1"]),
        "card1",
        "5b7c23d1d3025b6c\n",
    );
    assert!(collect(&host, &[]).is_empty());
}
