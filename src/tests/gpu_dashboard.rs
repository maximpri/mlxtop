use crate::test_support::*;
// SPDX-License-Identifier: MIT
use super::*;

#[test]
fn selection_pages_through_every_card_and_clamps_after_removal() {
    assert_eq!(visible(8, 2, 0), 0..2);
    assert_eq!(visible(8, 2, 3), 2..4);
    assert_eq!(visible(8, 2, 7), 6..8);
    assert_eq!(visible(2, 2, 7), 0..2);
    assert_eq!(visible(0, 2, 0), 0..0);
    assert_eq!(visible(8, 0, 3), 0..0);
}

#[test]
fn static_report_includes_all_card_identities_and_missing_readings() {
    let devices = gpu::parse(
        "0, GPU-a, NVIDIA A100, 98, 40960, 81920, 67\n1, GPU-b, NVIDIA A100, N/A, N/A, 81920, N/A",
    );
    let lines = static_lines(&devices, Thresholds::default());
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("GPU-a"));
    assert!(lines[0].contains("40.0/80.0 GiB"));
    assert!(lines[1].contains("GPU-b"));
    assert!(lines[1].contains("utilization —"));
    assert!(lines[1].contains("temperature —"));
}

#[test]
fn state_names_a_clock_limit_only_while_the_card_works() {
    let mut device = gpu::parse("0, GPU-a, NVIDIA RTX 4090, 95, 1024, 24564, 88").remove(0);
    device.throttle_reasons = Some(0x40);
    assert_eq!(state(&device, Thresholds::default()), "thermal limit");
    device.throttle_reasons = Some(0x4);
    assert_eq!(state(&device, Thresholds::default()), "power cap");
    device.utilization = Some(5);
    device.throttle_reasons = Some(0x40);
    assert_ne!(state(&device, Thresholds::default()), "thermal limit");
    assert!(static_lines(&[device], Thresholds::default())[0].ends_with("active"));
}

/// Two RTX 4090s with their limits read: one idle, one decoding.
fn two_cards() -> Vec<gpu::Device> {
    let mut cards = gpu::parse(
        "0, GPU-a, NVIDIA RTX 4090, 0, 1024, 24564, 35\n1, GPU-b, NVIDIA RTX 4090, 97, 22000, 24564, 78\n",
    );
    gpu::apply_limits(
        &mut cards,
        "GPU-a, 31.27, 450.00, 210, 3105, 0, 1, 4, 16, 16\n\
         GPU-b, 183.45, 450.00, 2520, 3105, 71, 4, 4, 16, 16\n",
        std::time::Instant::now(),
    );
    cards
}

/// A Strix Halo APU: no VRAM of its own, 58.4 GiB of RAM held of 112.5.
fn apu() -> gpu::Device {
    gpu::Device {
        index: 0,
        uuid: "AMD-0000:c5:00.0".into(),
        vendor: gpu::Vendor::Amd,
        name: "AMD Radeon Graphics / Radeon 8050S / 8060S Graphics".into(),
        utilization: Some(97),
        used: None,
        total: None,
        temperature: Some(64),
        throttle_reasons: None,
        unified: true,
        limits: gpu::Limits {
            power_mw: Some(86_000),
            clock_mhz: Some(2826),
            clock_max_mhz: Some(2900),
            ram_used: Some((58 << 30) + (384 << 20)),
            ram_limit: Some((112 << 30) + (512 << 20)),
            ..gpu::Limits::default()
        },
    }
}

fn table(devices: &[gpu::Device], width: u16) -> String {
    let height = devices.len() as u16 + 3;
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| draw(frame, frame.area(), devices, 1, Thresholds::default()))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_table_keeps_its_primary_readings_and_adds_limits_as_width_allows() {
    let cards = two_cards();
    // Column labels and cells by the terminal width they appear from: the
    // table's header row is four cells narrower than the terminal.
    let optional = [
        (114, ["POWER", "183/450 W"]),
        (160, ["CLOCK", "2520/3105 MHz"]),
        (160, ["MEM BUSY", "71%"]),
        (200, ["PCIE", "Gen4 x16"]),
    ];
    for width in [80, 100, 112, 114, 120, 160, 200] {
        let screen = table(&cards, width);
        for primary in [
            "UTILIZATION",
            "VRAM USED / TOTAL",
            "TEMP",
            "97%",
            "21.5/24.0 GiB",
            "78°C",
        ] {
            assert!(screen.contains(primary), "{primary} at {width}\n{screen}");
        }
        for (from, texts) in optional {
            for text in texts {
                assert_eq!(
                    screen.contains(text),
                    width >= from,
                    "{text} at {width}\n{screen}"
                );
            }
        }
    }
    // A reading no card reports gets no column of dashes.
    let bare = gpu::parse("0, GPU-a, NVIDIA RTX 4090, 97, 22000, 24564, 78\n");
    let screen = table(&bare, 200);
    for label in ["POWER", "CLOCK", "MEM BUSY", "PCIE"] {
        assert!(!screen.contains(label), "{label}\n{screen}");
    }
    // A card without a reading another card has shows a dash in its place.
    let mut mixed = two_cards();
    mixed[0].limits = gpu::Limits::default();
    let screen = table(&mixed, 200);
    let idle_row = screen.lines().nth(2).unwrap();
    assert_eq!(idle_row.matches('—').count(), 4, "{idle_row}");
}

#[test]
fn a_gpu_without_memory_of_its_own_shows_the_ram_it_holds() {
    for width in [80, 160, 200] {
        let screen = table(&[apu()], width);
        assert!(screen.contains("RAM USED / LIMIT"), "{screen}");
        assert!(screen.contains("58.4/112.5 GiB"), "{screen}");
        assert!(!screen.contains("VRAM"), "{screen}");
        // No slot to report.
        assert!(!screen.contains("PCIE"), "{screen}");
    }
    let wide = table(&[apu()], 200);
    assert!(wide.contains("RAM LOAD") && wide.contains("51%"), "{wide}");
    assert!(
        wide.contains("86 W") && wide.contains("2826/2900 MHz"),
        "{wide}"
    );
    // DGX Spark: what its processes hold; unknown until they are read. Its
    // limit is all of RAM, which the memory panel shows: no bar of its own.
    let mut spark = gpu::parse("0, GPU-s, NVIDIA GB10, 96, [N/A], [N/A], 71\n");
    assert_eq!(memory_label(&spark[0]), "—");
    spark[0].limits.ram_used = Some(61 << 30);
    assert_eq!(memory_label(&spark[0]), "61.0 GiB");
    let screen = table(&spark, 200);
    assert!(
        screen.contains("RAM USED ") && screen.contains("61.0 GiB"),
        "{screen}"
    );
    assert!(
        !screen.contains("LIMIT") && !screen.contains("LOAD"),
        "{screen}"
    );
}

#[test]
fn static_lines_give_every_reading_a_card_reports() {
    let lines = static_lines(&two_cards(), Thresholds::default());
    assert_eq!(
        lines[1],
        "GPU 1       NVIDIA RTX 4090 · GPU-b · utilization 97% · VRAM 21.5/24.0 GiB · temperature 78°C · power 183/450 W · clock 2520/3105 MHz · memory busy 71% · PCIe Gen4 x16 · saturated"
    );
    // An idle card's slowed link is told beside what it allows.
    assert!(
        lines[0].contains("PCIe Gen1 x16 of Gen4 x16 · idle"),
        "{}",
        lines[0]
    );
    let apu = static_lines(&[apu()], Thresholds::default()).remove(0);
    assert!(apu.contains("RAM 58.4/112.5 GiB"), "{apu}");
    assert!(!apu.contains("PCIe") && !apu.contains("GTT"), "{apu}");
    // A card with VRAM of its own: the system RAM it maps besides.
    let mut card = gpu::parse("0, GPU-a, NVIDIA RTX 4090, 50, 1024, 24564, 35\n").remove(0);
    card.vendor = gpu::Vendor::Amd;
    card.limits.ram_used = Some(768 << 20);
    card.limits.ram_limit = Some(33 << 30);
    let line = static_lines(&[card], Thresholds::default()).remove(0);
    assert!(line.contains("GTT 0.8/33.0 GiB · active"), "{line}");
}
