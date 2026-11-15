// SPDX-License-Identifier: MIT
use super::*;
use crate::domain::MetalTelemetry;

const GIB: u64 = 1 << 30;

/// The figure for a chip, with its memory in GiB, as a plain number.
fn gb_s(name: &str, cores: Option<u16>, memory_gib: Option<u64>) -> Option<f64> {
    lookup(name, cores, memory_gib.map(|gib| gib * GIB)).map(|bandwidth| bandwidth.gb_s)
}

fn mac(name: &str, cores: Option<u16>, memory: u64) -> Sample {
    Sample {
        total_memory: memory,
        metal: MetalTelemetry {
            device_name: Some(name.into()),
            gpu_cores: cores,
            ..MetalTelemetry::default()
        },
        ..Sample::default()
    }
}

#[test]
fn the_cut_down_m4_max_reads_410_gb_s_not_the_546_of_the_full_chip() {
    // The owner's Mac Studio (Mac16,9), as ioreg and sysctl report it.
    let studio = mac("Apple M4 Max", Some(32), 38_654_705_664);
    assert_eq!(of_sample(&studio), Some(Bandwidth { gb_s: 410.0 }));
    assert_eq!(of_sample(&studio).unwrap().label(), "410 GB/s");
    let full = mac("Apple M4 Max", Some(40), 128 * GIB);
    assert_eq!(of_sample(&full).unwrap().label(), "546 GB/s");
}

#[test]
fn the_gpu_core_count_picks_the_bin_and_ram_decides_where_it_matters() {
    assert_eq!(gb_s("Apple M3 Max", Some(30), Some(36)), Some(300.0));
    assert_eq!(gb_s("Apple M3 Max", Some(40), Some(48)), Some(400.0));
    assert_eq!(gb_s("Apple M5 Max", Some(32), Some(36)), Some(460.0));
    assert_eq!(gb_s("Apple M5 Max", Some(40), Some(128)), Some(614.0));
    assert_eq!(gb_s("Apple M5 Ultra", Some(80), Some(512)), Some(1200.0));
    assert_eq!(gb_s("Apple M5", Some(8), Some(16)), Some(153.0));
    // M6 reads 153 GB/s with 16 GB and 170 GB/s with 24 or 32 GB.
    assert_eq!(gb_s("Apple M6", Some(12), Some(16)), Some(153.0));
    assert_eq!(gb_s("Apple M6", Some(12), Some(24)), Some(170.0));
    assert_eq!(gb_s("Apple M6", Some(12), Some(32)), Some(170.0));
    // hw.memsize is exact; a size Apple does not sell, or none, is unknown.
    assert_eq!(gb_s("Apple M6", Some(12), Some(8)), None);
    assert_eq!(gb_s("Apple M6", Some(12), None), None);
    assert_eq!(of_sample(&mac("Apple M6", Some(12), 0)), None);
    // The iMac's 10-core M6 has no published figure per memory size.
    assert_eq!(gb_s("Apple M6", Some(10), Some(24)), None);
}

#[test]
fn a_chip_or_bin_the_readings_cannot_settle_is_unknown() {
    // Without its core count, no Mac chip is guessed, even where every bin
    // of it reads the same.
    assert_eq!(gb_s("Apple M4 Max", None, Some(36)), None);
    assert_eq!(gb_s("Apple M4", None, Some(16)), None);
    // A core count Apple never sold.
    assert_eq!(gb_s("Apple M4 Max", Some(36), Some(36)), None);
    // Chips the table does not know, and ioreg's fallback plugin name.
    for name in [
        "Apple M9 Max",
        "Apple Paravirtual device",
        "AGXMetalG16X",
        "",
        "Apple",
    ] {
        assert_eq!(gb_s(name, Some(32), Some(36)), None, "{name}");
    }
    assert_eq!(of_sample(&Sample::default()), None);
}

#[test]
fn cards_are_known_by_the_names_their_drivers_give() {
    // nvidia-smi's names, as the fixtures record them; older drivers leave
    // out "NVIDIA", and the consumer cards' "GeForce" is optional.
    let rtx = gpu::parse("0, GPU-a, NVIDIA RTX 4090, 64, 12000, 24564, 61\n").remove(0);
    assert_eq!(card(&rtx).unwrap().label(), "1008 GB/s");
    let windows = gpu::parse("0, GPU-w, NVIDIA GeForce RTX 4080, 91, 15900, 16376, 83\n").remove(0);
    assert_eq!(card(&windows), Some(Bandwidth { gb_s: 716.8 }));
    assert_eq!(card(&windows).unwrap().label(), "717 GB/s");
    assert_eq!(gb_s("GeForce RTX 3090", None, Some(24)), Some(936.0));
    // DGX Spark reports no memory of its own.
    let spark = gpu::parse("0, GPU-s, NVIDIA GB10, 42, [N/A], [N/A], 51\n").remove(0);
    assert_eq!(card(&spark).unwrap().label(), "273 GB/s");
    // The form factor sets an A100's bandwidth; a bare name could be either.
    let a100 = gpu::parse("0, GPU-a, NVIDIA A100, 98, 40960, 81920, 67\n").remove(0);
    assert_eq!(card(&a100), None);
    assert_eq!(gb_s("NVIDIA A100-SXM4-80GB", None, Some(80)), Some(2039.0));
    assert_eq!(gb_s("NVIDIA A100 80GB PCIe", None, Some(80)), Some(1935.0));
    assert_eq!(gb_s("NVIDIA H100 80GB HBM3", None, Some(80)), Some(3350.0));
    // Another card of a similar name is another card.
    for name in [
        "NVIDIA GeForce RTX 4090 Laptop GPU",
        "NVIDIA GeForce RTX 4080 SUPER",
        "NVIDIA RTX 4090 #2",
    ] {
        assert_eq!(gb_s(name, None, Some(16)), None, "{name}");
    }
    // One name, two memories: memory tells them apart, or nothing does.
    assert_eq!(gb_s("NVIDIA GeForce RTX 3080", None, Some(10)), Some(760.0));
    assert_eq!(gb_s("NVIDIA GeForce RTX 3080", None, Some(12)), None);
    assert_eq!(gb_s("NVIDIA GeForce RTX 3080", None, None), None);
    assert_eq!(gb_s("NVIDIA GeForce RTX 4070", None, Some(12)), None);

    // amdgpu's names come from pci.ids, as gpu_amd's fixture builds them,
    // with the memory it reads: a little under 16 GiB.
    let navi48 = "AMD Radeon RX 9070/9070 XT/9070 GRE";
    let reading = lookup(navi48, None, Some(17_095_983_104));
    assert_eq!(reading.map(Bandwidth::label).as_deref(), Some("640 GB/s"));
    assert_eq!(gb_s(navi48, None, Some(12)), Some(432.0));
    assert_eq!(gb_s(navi48, None, None), None);
    let navi31 = "AMD Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M";
    assert_eq!(gb_s(navi31, None, Some(24)), Some(960.0));
    assert_eq!(gb_s(navi31, None, Some(20)), Some(800.0));
    assert_eq!(gb_s(navi31, None, Some(16)), None);
    // A card the driver names on its own needs no memory to tell.
    assert_eq!(gb_s("AMD Radeon RX 7900 XTX", None, None), Some(960.0));
    // Strix Halo's pci.ids name covers a 128-bit part too.
    let strix = "AMD Radeon Graphics / Radeon 8050S Graphics / Radeon 8060S Graphics";
    assert_eq!(gb_s(strix, None, Some(96)), None);
    assert_eq!(gb_s("AMD Radeon 8060S Graphics", None, None), Some(256.0));
    assert_eq!(gb_s("AMD Radeon 8040S Graphics", None, None), Some(128.0));
    for name in [
        "AMD Radeon Test",
        "AMD Instinct MI300X",
        "AMD GPU 1002:7550",
    ] {
        assert_eq!(gb_s(name, None, Some(16)), None, "{name}");
    }
}

#[test]
fn the_host_footer_names_one_chip() {
    let mut sample = mac("Apple M4 Max", Some(32), 36 * GIB);
    let rtx = gpu::parse("0, GPU-a, NVIDIA RTX 4090, 64, 12000, 24564, 61\n").remove(0);
    sample.gpus = vec![rtx.clone()];
    if sample.has_discrete_gpus() {
        assert_eq!(of_sample(&sample).unwrap().label(), "1008 GB/s");
        // Several cards have no single figure.
        sample.gpus.push(rtx);
        assert_eq!(of_sample(&sample), None);
    } else {
        // A Mac has no discrete cards: its chip is the one.
        assert_eq!(of_sample(&sample).unwrap().label(), "410 GB/s");
    }
}

#[test]
fn no_reading_matches_two_rows() {
    // Both tables as (name, GPU cores, memory GiB, GB/s); a card has no bins.
    let rows: Vec<(&str, Vec<u64>, &[u64], f64)> = APPLE
        .iter()
        .map(|&(chip, cores, ram, gb_s)| {
            (chip, cores.iter().map(|&n| n.into()).collect(), ram, gb_s)
        })
        .chain(
            CARDS
                .iter()
                .map(|&(card, sizes, gb_s)| (card, Vec::new(), sizes, gb_s)),
        )
        .collect();
    let overlap =
        |a: &[u64], b: &[u64]| a.is_empty() || b.is_empty() || a.iter().any(|x| b.contains(x));
    for (index, (name, cores, memory, gb_s)) in rows.iter().enumerate() {
        assert!(gb_s.is_finite() && *gb_s > 0.0, "{name}");
        assert_eq!(&key(name), name, "{name} is not in key form");
        for (other, other_cores, other_memory, _) in &rows[index + 1..] {
            assert!(
                !(name.eq_ignore_ascii_case(other)
                    && overlap(cores, other_cores)
                    && overlap(memory, other_memory)),
                "two rows hold for one {name}"
            );
        }
    }
    // Every Mac chip is known only with its bin.
    assert!(APPLE.iter().all(|(_, cores, _, _)| !cores.is_empty()));
}

#[test]
fn every_row_cites_its_source() {
    // A row follows a comment block that holds a link, alone or with the
    // other rows its source covers.
    let source = include_str!("../chip_facts.rs");
    let mut rows = 0;
    let mut in_table = false;
    let mut cited = false;
    let mut in_comment = false;
    for line in source.lines().map(str::trim) {
        if line.starts_with("const APPLE: &[") || line.starts_with("const CARDS: &[") {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }
        if line == "];" {
            in_table = false;
        } else if line.is_empty() {
            cited = false;
            in_comment = false;
        } else if line.starts_with("//") {
            cited = (cited && in_comment) || line.contains("https://");
            in_comment = true;
        } else {
            in_comment = false;
            assert!(cited, "no source for `{line}`");
            rows += usize::from(line.starts_with("(\""));
        }
    }
    assert_eq!(rows, APPLE.len() + CARDS.len());
}
