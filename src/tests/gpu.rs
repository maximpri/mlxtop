// SPDX-License-Identifier: MIT
use super::*;
use std::time::{Duration, Instant};

const TWO: &str = "1, GPU-b, NVIDIA RTX 4090, 97, 22000, 24564, 78\n0, GPU-a, NVIDIA RTX 4090, 0, 1024, 24564, 35\n";

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
#[test]
fn nvidia_collection_is_disabled_on_macos() {
    let host = crate::test_support::FakeHost::default();
    assert!(collect(&host, &parse(TWO)).is_empty());
}

#[test]
fn keeps_every_card_sorted_without_merging_identical_models() {
    let cards = parse(TWO);
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0].index, 0);
    assert_eq!(cards[0].uuid, "GPU-a");
    assert_eq!(cards[1].uuid, "GPU-b");
    assert_eq!(cards[1].temperature, Some(78));
    assert_eq!(cards[1].used, Some(22000 * MIB));
    assert_eq!(peak_utilization(&cards), Some(97));
    assert_eq!(memory_totals(&cards), Some((23024 * MIB, 49128 * MIB)));
}

#[test]
fn unsupported_fields_and_bad_rows_do_not_become_zero_or_hide_other_cards() {
    let cards = parse(&format!(
        "garbage\n{TWO}2, GPU-c, \"NVIDIA, test\", [N/A], N/A, 8192, [Not Supported]\n"
    ));
    assert_eq!(cards.len(), 3);
    assert_eq!(cards[0].utilization, Some(0));
    assert_eq!(cards[2].name, "NVIDIA, test");
    assert_eq!(cards[2].utilization, None);
    assert_eq!(cards[2].used, None);
    assert_eq!(cards[2].temperature, None);
    assert_eq!(peak_utilization(&cards), None);
    assert_eq!(memory_totals(&cards), None);
    let bad = parse("0, GPU-a, Test, 101, -1, 18446744073709551615, N/A");
    assert_eq!(bad[0].utilization, None);
    assert_eq!(bad[0].used, None);
    assert_eq!(bad[0].total, None);
    assert_eq!(bad[0].memory_percent(), None);
}

#[test]
fn failed_poll_clears_counters_and_recovers_by_uuid() {
    let original = parse(TWO);
    for output in [None, Some(""), Some("Failed to initialize NVML")] {
        let missing = readings(output, &original);
        assert_eq!(missing.len(), 2);
        assert_eq!(missing[0].uuid, original[0].uuid);
        assert_eq!(missing[1].utilization, None);
        assert_eq!(missing[1].used, None);
        assert_eq!(missing[1].total, None);
        assert_eq!(missing[1].temperature, None);
        assert_eq!(readings(Some(TWO), &missing), original);
    }
    let removed = readings(
        Some("3, GPU-b, NVIDIA RTX 4090, 50, 2000, 24564, 45"),
        &original,
    );
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].uuid, "GPU-b");
    assert_eq!(removed[0].index, 3);
    assert_eq!(parse(&format!("{TWO}{TWO}")).len(), 2);
    assert_eq!(peak_utilization(&[]), None);
}

#[test]
fn throttle_reasons_attach_by_uuid_and_rank_faults_before_power_cap() {
    let mut cards = parse(TWO);
    apply_throttle_reasons(
        &mut cards,
        "GPU-a, 0x0000000000000001\nGPU-b, 0x0000000000000044\nGPU-z, 0x20\nbad\nGPU-a, [N/A]\n",
    );
    // Idle alone is not a limit; thermal outranks the power cap beside it.
    assert_eq!(cards[0].throttle_reasons, Some(1));
    assert_eq!(cards[0].throttle(), None);
    assert_eq!(cards[1].throttle(), Some(Throttle::Thermal));
    assert!(Throttle::Thermal.is_fault());
    assert!(!Throttle::PowerCap.is_fault());
    for (mask, cause) in [
        (0x80, Throttle::PowerBrake),
        (0x08, Throttle::HardwareSlowdown),
        (0x04, Throttle::PowerCap),
        (0x24, Throttle::Thermal),
    ] {
        cards[0].throttle_reasons = Some(mask);
        assert_eq!(cards[0].throttle(), Some(cause), "{mask:#x}");
    }
    // A failed poll does not keep a stale limit.
    assert_eq!(readings(None, &cards)[1].throttle_reasons, None);
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[test]
fn throttle_query_falls_back_to_the_newer_field_name_and_skips_without_cards() {
    use crate::test_support::FakeHost;
    let host = FakeHost::default()
        .command(CORE, TWO)
        .command(NEW_REASONS, "GPU-b, 0x0000000000000040\n");
    let cards = collect(&host, &[]);
    assert_eq!(cards[1].throttle(), Some(Throttle::Thermal));
    assert_eq!(cards[0].throttle_reasons, None);
    // The core counters, both names of the reasons field, and the limits;
    // neither card is nearly full, so no process query.
    let calls = host.calls();
    assert_eq!(calls, [CORE, OLD_REASONS, NEW_REASONS, LIMITS], "{calls:?}");
    let empty = FakeHost::default();
    assert!(collect(&empty, &[]).is_empty());
    assert_eq!(empty.calls().len(), 1);
}

const CORE: &str = "nvidia-smi --query-gpu=index,uuid,name,utilization.gpu,memory.used,memory.total,temperature.gpu --format=csv,noheader,nounits";
const OLD_REASONS: &str =
    "nvidia-smi --query-gpu=uuid,clocks_throttle_reasons.active --format=csv,noheader";
const NEW_REASONS: &str =
    "nvidia-smi --query-gpu=uuid,clocks_event_reasons.active --format=csv,noheader";
const LIMITS: &str = "nvidia-smi --query-gpu=uuid,power.draw,enforced.power.limit,clocks.sm,clocks.max.sm,utilization.memory,pcie.link.gen.current,pcie.link.gen.max,pcie.link.width.current,pcie.link.width.max --format=csv,noheader,nounits";
const PROCESSES: &str =
    "nvidia-smi --query-compute-apps=gpu_uuid,pid,used_memory --format=csv,noheader,nounits";

/// Two RTX 4090s, one idle and one decoding, as `nvidia-smi` prints them.
const TWO_LIMITS: &str = "GPU-a, 31.27, 450.00, 210, 3105, 0, 1, 4, 16, 16\n\
GPU-b, 183.45, 450.00, 2520, 3105, 71, 4, 4, 16, 16\n";

/// DGX Spark: the GB10 answers utilization and temperature and reports its
/// device memory as not supported, by design.
const GB10: &str = "0, GPU-s, NVIDIA GB10, 96, [N/A], [N/A], 71\n";
const GB10_NOT_SUPPORTED: &str =
    "0, GPU-s, NVIDIA GB10, 96, [Not Supported], [Not Supported], 71\n";

fn host_with(core: &str, limits: Option<&str>) -> crate::test_support::FakeHost {
    let host = crate::test_support::FakeHost::default().command(CORE, core);
    match limits {
        Some(limits) => host.command(LIMITS, limits),
        None => host,
    }
}

#[test]
fn limits_read_power_clocks_memory_activity_and_link_by_uuid() {
    let now = Instant::now();
    let host = host_with(TWO, Some(TWO_LIMITS));
    let cards = nvidia(&host, &[], true, now);
    let busy = &cards[1].limits;
    assert_eq!(busy.power_mw, Some(183_450));
    assert_eq!(busy.power_cap_mw, Some(450_000));
    assert_eq!(
        (busy.clock_mhz, busy.clock_max_mhz),
        (Some(2520), Some(3105))
    );
    assert_eq!(busy.memory_busy, Some(71));
    assert_eq!((busy.link_gen, busy.link_gen_max), (Some(4), Some(4)));
    assert_eq!((busy.link_width, busy.link_width_max), (Some(16), Some(16)));
    assert_eq!(busy.next_read, Some(now + LIMITS_INTERVAL));
    // An idle card: a measured 0% memory activity, its link slowed to Gen1.
    let idle = &cards[0].limits;
    assert_eq!(idle.memory_busy, Some(0));
    assert_eq!(idle.link_gen, Some(1));
    assert!(!cards[0].link_reduced(), "an idle card slows its link");
    assert_eq!(board_power_mw(&cards), Some(214_720));
    assert_eq!(power_cap_mw(&cards), Some(900_000));
    assert_eq!(power_label(Some(214_720), Some(900_000)), "215/900 W");
    // The core counters are those of the core query alone.
    let core = parse(TWO);
    for (card, core) in cards.iter().zip(&core) {
        assert_eq!(
            (card.utilization, card.used, card.total, card.temperature),
            (core.utilization, core.used, core.total, core.temperature)
        );
    }
}

#[test]
fn na_and_not_supported_read_as_unknown_field_by_field() {
    for core in [GB10, GB10_NOT_SUPPORTED] {
        let host = host_with(
            core,
            Some("GPU-s, 37.12, [N/A], 2405, [Not Supported], [N/A], [N/A], Not Supported, , [Unknown Error]\n"),
        );
        let cards = nvidia(&host, &[], false, Instant::now());
        let spark = &cards[0];
        assert_eq!((spark.used, spark.total), (None, None), "{core}");
        assert!(spark.unified, "no device memory: it works in RAM");
        assert_eq!(memory(&cards), Memory::Shared);
        assert_eq!(memory_totals(&cards), None);
        assert_eq!(spark.utilization, Some(96));
        let limits = &spark.limits;
        assert_eq!(limits.power_mw, Some(37_120));
        assert_eq!(limits.power_cap_mw, None);
        assert_eq!((limits.clock_mhz, limits.clock_max_mhz), (Some(2405), None));
        assert_eq!(limits.memory_busy, None);
        assert_eq!(
            (
                limits.link_gen,
                limits.link_gen_max,
                limits.link_width,
                limits.link_width_max
            ),
            (None, None, None, None)
        );
        assert_eq!(power_label(limits.power_mw, limits.power_cap_mw), "37 W");
    }
    // Zero, negative, out-of-range and non-numeric readings are unknown too.
    let mut cards = parse(TWO);
    apply_limits(
        &mut cards,
        "GPU-a, 0.00, -5, 0, 99999, 101, 0, 12, 0, 128\nGPU-b, NaN, inf, x, , 50, 4\n",
        Instant::now(),
    );
    assert_eq!(
        cards[0].limits,
        Limits {
            next_read: cards[0].limits.next_read,
            ..Limits::default()
        }
    );
    // A short row is not attached at all.
    assert_eq!(cards[1].limits.memory_busy, None);
    assert_eq!(watts("183.45"), Some(183_450));
}

#[test]
fn a_bad_field_in_the_limits_query_leaves_the_core_readings_intact() {
    // A driver that lacks one of the fields fails the whole query (a nonzero
    // exit, so no output), or prints a complaint instead of rows.
    for limits in [
        None,
        Some("Field \"pcie.link.gen.max\" is not a valid field to query.\n"),
    ] {
        let host = host_with(TWO, limits).command(NEW_REASONS, "GPU-b, 0x0000000000000040\n");
        let now = Instant::now();
        let cards = nvidia(&host, &[], true, now);
        // A query that failed outright is retried a minute later, so its
        // warning does not fill the log; a complaint is read as rows were.
        let next = now
            + if limits.is_none() {
                LIMITS_RETRY
            } else {
                LIMITS_INTERVAL
            };
        let core = parse(TWO);
        assert_eq!(cards.len(), 2);
        for (card, core) in cards.iter().zip(&core) {
            assert_eq!(
                (
                    card.uuid.as_str(),
                    card.utilization,
                    card.used,
                    card.total,
                    card.temperature
                ),
                (
                    core.uuid.as_str(),
                    core.utilization,
                    core.used,
                    core.total,
                    core.temperature
                )
            );
            // Unknown, and not asked again until the next read is due.
            assert_eq!(
                card.limits,
                Limits {
                    next_read: Some(next),
                    ..Limits::default()
                }
            );
        }
        assert_eq!(cards[1].throttle(), Some(Throttle::Thermal));
        assert_eq!(
            memory(&cards),
            Memory::Dedicated {
                used: 23024 * MIB,
                total: 49128 * MIB
            }
        );
        assert_eq!(board_power_mw(&cards), None);
        // Each query ran on its own; the failed one took nothing with it.
        assert_eq!(host.calls(), [CORE, OLD_REASONS, NEW_REASONS, LIMITS]);
        let limit_reads = || host.calls().iter().filter(|call| *call == LIMITS).count();
        nvidia(&host, &cards, true, now + LIMITS_INTERVAL);
        assert_eq!(limit_reads(), if limits.is_none() { 1 } else { 2 });
        nvidia(&host, &cards, true, now + LIMITS_RETRY);
        assert_eq!(limit_reads(), if limits.is_none() { 2 } else { 3 });
    }
}

#[test]
fn limits_are_read_every_few_seconds_and_carried_between() {
    let t0 = Instant::now();
    let host = host_with(TWO, Some(TWO_LIMITS)).command(
        LIMITS,
        "GPU-a, 40.00, 450.00, 300, 3105, 0, 1, 4, 16, 16\n\
         GPU-b, 420.00, 450.00, 2400, 3105, 88, 4, 4, 16, 16\n",
    );
    let limit_reads = |host: &crate::test_support::FakeHost| {
        host.calls().iter().filter(|call| *call == LIMITS).count()
    };
    let first = nvidia(&host, &[], true, t0);
    assert_eq!(limit_reads(&host), 1);
    // A second later: carried, not read.
    let second = nvidia(&host, &first, true, t0 + Duration::from_secs(1));
    assert_eq!(limit_reads(&host), 1);
    assert_eq!(second[1].limits, first[1].limits);
    // Five seconds on: read again.
    let third = nvidia(&host, &second, true, t0 + LIMITS_INTERVAL);
    assert_eq!(limit_reads(&host), 2);
    assert_eq!(third[1].limits.power_mw, Some(420_000));
    assert_eq!(third[1].limits.next_read, Some(t0 + 2 * LIMITS_INTERVAL));
    // A card not seen before is read at once.
    let mut fewer = third.clone();
    fewer.remove(0);
    nvidia(
        &host,
        &fewer,
        true,
        t0 + LIMITS_INTERVAL + Duration::from_secs(1),
    );
    assert_eq!(limit_reads(&host), 3);
    // A failed core poll forgets them, and the next good poll reads again.
    let failed = readings(None, &third);
    assert_eq!(failed[1].limits, Limits::default());
    let back = host_with(TWO, Some(TWO_LIMITS));
    let recovered = nvidia(&back, &failed, true, t0 + Duration::from_secs(12));
    assert_eq!(limit_reads(&back), 1);
    assert_eq!(recovered[1].limits.power_mw, Some(183_450));
}

#[test]
fn process_memory_is_read_where_it_answers_a_question() {
    let now = Instant::now();
    // DGX Spark: what its processes hold is the RAM the GPU holds.
    let spark = host_with(GB10, None).command(PROCESSES, "GPU-s, 4242, 61440\nGPU-s, 777, 2048\n");
    let cards = nvidia(&spark, &[], true, now);
    assert_eq!(cards[0].limits.ram_used, Some(63_488 * MIB));
    assert_eq!(
        cards[0].limits.processes,
        vec![(4242, 61_440 * MIB), (777, 2048 * MIB)]
    );
    assert_eq!(shared_in_use(&cards), Some(63_488 * MIB));
    // No compute process at all: it holds nothing.
    let quiet = host_with(GB10, None).command(PROCESSES, "");
    assert_eq!(nvidia(&quiet, &[], true, now)[0].limits.ram_used, Some(0));
    // One process whose memory is not reported leaves the sum unknown.
    let partial =
        host_with(GB10, None).command(PROCESSES, "GPU-s, 4242, 61440\nGPU-s, 777, [N/A]\n");
    let cards = nvidia(&partial, &[], true, now);
    assert_eq!(cards[0].limits.ram_used, None);
    assert_eq!(cards[0].limits.processes, vec![(4242, 61_440 * MIB)]);
    assert_eq!(shared_in_use(&cards), None);
    // Windows keeps per-process memory to itself: not asked.
    let windows = host_with(GB10, None).command(PROCESSES, "GPU-s, 4242, 61440\n");
    assert_eq!(nvidia(&windows, &[], false, now)[0].limits.ram_used, None);
    assert!(!windows.calls().iter().any(|call| call == PROCESSES));
    // A card with room to spare: not asked.
    let roomy = host_with(TWO, None).command(PROCESSES, "GPU-b, 1, 1024\n");
    nvidia(&roomy, &[], true, now);
    assert!(!roomy.calls().iter().any(|call| call == PROCESSES));
    // A card nearly full: who holds it, largest first. Its RAM stays unknown.
    // The card beside it with room to spare keeps no list of its own.
    let full = host_with(
        "0, GPU-f, NVIDIA RTX 4090, 97, 23900, 24564, 70\n1, GPU-g, NVIDIA RTX 4090, 50, 12000, 24564, 60\n",
        None,
    )
    .command(
        PROCESSES,
        "GPU-f, 912, 4096\nGPU-f, 77, 18432\nGPU-g, 77, 11000\nGPU-x, 5, 100\nbad\n",
    );
    let cards = nvidia(&full, &[], true, now);
    assert_eq!(
        cards[0].limits.processes,
        vec![(77, 18_432 * MIB), (912, 4096 * MIB)]
    );
    assert_eq!(cards[0].limits.ram_used, None);
    assert!(cards[1].limits.processes.is_empty());
    assert_eq!(shared_in_use(&cards), None);
}

#[test]
fn a_failed_core_query_asks_nothing_else() {
    let host = crate::test_support::FakeHost::default().command(LIMITS, TWO_LIMITS);
    let previous = parse(TWO);
    let cards = nvidia(&host, &previous, true, Instant::now());
    // Known cards stay listed, with nothing read.
    assert_eq!(cards.len(), 2);
    assert!(cards
        .iter()
        .all(|gpu| gpu.utilization.is_none() && gpu.limits == Limits::default()));
    assert_eq!(host.calls(), [CORE]);
    // A row for a card the core query did not list is not attached.
    let mut cards = parse(TWO);
    apply_limits(
        &mut cards,
        "GPU-z, 99.00, 450.00, 2000, 3105, 50, 4, 4, 16, 16\n",
        Instant::now(),
    );
    assert!(cards.iter().all(|gpu| gpu.limits.power_mw.is_none()));
}

#[test]
fn an_apu_is_listed_only_without_a_card_of_its_own() {
    let apu = Device {
        index: 0,
        uuid: "AMD-apu".into(),
        vendor: Vendor::Amd,
        name: "AMD Radeon 8060S".into(),
        utilization: Some(5),
        used: None,
        total: None,
        temperature: Some(50),
        throttle_reasons: None,
        unified: true,
        limits: Limits::default(),
    };
    let dgpu = Device {
        index: 1,
        uuid: "AMD-card".into(),
        unified: false,
        total: Some(16 << 30),
        used: Some(1 << 30),
        ..apu.clone()
    };
    // Beside an NVIDIA card or an AMD card of its own, the APU drives the
    // desktop: left out, and the others are numbered after NVIDIA's.
    let merged = merge(parse(TWO), vec![apu.clone(), dgpu.clone()]);
    assert_eq!(
        merged
            .iter()
            .map(|gpu| (gpu.index, gpu.uuid.as_str()))
            .collect::<Vec<_>>(),
        [(0, "GPU-a"), (1, "GPU-b"), (2, "AMD-card")]
    );
    let merged = merge(Vec::new(), vec![apu.clone(), dgpu]);
    assert_eq!(merged.len(), 1);
    assert_eq!((merged[0].index, merged[0].uuid.as_str()), (0, "AMD-card"));
    // On its own it is the GPU, and memory is one pool.
    let merged = merge(Vec::new(), vec![apu]);
    assert_eq!(merged.len(), 1);
    assert_eq!(memory(&merged), Memory::Shared);
}

#[test]
fn limit_readings_explain_themselves_only_on_a_busy_card() {
    let mut card = parse(TWO).remove(1);
    card.limits = Limits {
        clock_mhz: Some(1140),
        clock_max_mhz: Some(2790),
        link_gen: Some(3),
        link_gen_max: Some(4),
        link_width: Some(8),
        link_width_max: Some(16),
        ..Limits::default()
    };
    assert!(card.busy());
    assert_eq!(card.clock_held(), Some(40));
    assert!(card.link_reduced());
    // Boost keeps a working card near its top clock: not held.
    card.limits.clock_mhz = Some(2520);
    assert_eq!(card.clock_held(), None);
    // An idle card lowers both on purpose.
    card.limits.clock_mhz = Some(210);
    card.utilization = Some(3);
    assert!(!card.busy());
    assert_eq!(card.clock_held(), None);
    assert!(!card.link_reduced());
    // An integrated GPU has no slot.
    card.utilization = Some(97);
    card.unified = true;
    assert!(!card.link_reduced());
    // A thermal slowdown is the correlation engine's heat input; a power
    // cap is the card working as configured.
    let mut cards = parse(TWO);
    assert!(!heat_limited(&cards));
    for (mask, heat) in [
        (0x40, true),
        (0x20, true),
        (0x24, true),
        (0x4, false),
        (0x80, false),
    ] {
        cards[0].throttle_reasons = Some(mask);
        assert_eq!(heat_limited(&cards), heat, "{mask:#x}");
    }
}

#[test]
fn labels_say_what_is_known() {
    assert_eq!(power_label(Some(183_450), Some(450_000)), "183/450 W");
    assert_eq!(power_label(Some(86_000), None), "86 W");
    assert_eq!(power_label(None, Some(450_000)), "—");
    assert_eq!(clock_label(Some(2520), Some(3105)), "2520/3105 MHz");
    assert_eq!(clock_label(Some(2520), None), "2520 MHz");
    assert_eq!(clock_label(None, Some(3105)), "—");
    assert_eq!(link_label(Some(4), Some(16)), "Gen4 x16");
    assert_eq!(link_label(Some(4), None), "Gen4");
    assert_eq!(link_label(None, Some(8)), "x8");
    assert_eq!(link_label(None, None), "—");
    // A sum over the cards is all of them or unknown.
    let mut cards = parse(TWO);
    assert_eq!(board_power_mw(&[]), None);
    assert_eq!(power_cap_mw(&[]), None);
    cards[0].limits.power_mw = Some(30_000);
    assert_eq!(board_power_mw(&cards), None);
    cards[1].limits.power_mw = Some(180_000);
    assert_eq!(board_power_mw(&cards), Some(210_000));
    assert_eq!(power_cap_mw(&cards), None);
    assert_eq!(
        shared_in_use(&cards),
        None,
        "cards with VRAM hold no RAM pool"
    );
    assert_eq!(shared_in_use(&[]), None);
    assert_eq!(cards[0].ram_percent(), None);
    cards[0].limits.ram_used = Some(3 << 30);
    cards[0].limits.ram_limit = Some(12 << 30);
    assert_eq!(cards[0].ram_percent(), Some(25));
}

#[test]
fn vendor_labels_follow_each_backend_identity() {
    let mut cards = parse(TWO);
    assert_eq!(cards[0].vendor, Vendor::Nvidia);
    assert_eq!(vendors(&cards), "NVIDIA");
    assert_eq!(source(&cards), "nvidia-smi");
    let mut amd = cards[0].clone();
    amd.uuid = "AMD-5b7c23d1d3025b6c".into();
    amd.vendor = Vendor::Amd;
    cards.push(amd.clone());
    assert_eq!(vendors(&cards), "NVIDIA + AMD");
    assert_eq!(source(&cards), "nvidia-smi + amdgpu sysfs");
    assert_eq!(vendors(&[amd.clone()]), "AMD");
    assert_eq!(source(&[amd]), "amdgpu sysfs");
    assert_eq!(vendors(&[]), "");
}

#[cfg(target_os = "linux")]
#[test]
fn amd_cards_follow_nvidia_cards_in_one_list() {
    let dev = "/sys/class/drm/card1/device";
    let host = crate::test_support::FakeHost::default()
        .command(
            "nvidia-smi --query-gpu=index,uuid,name,utilization.gpu,memory.used,memory.total,temperature.gpu --format=csv,noheader,nounits",
            TWO,
        )
        .dir("/sys/class/drm", &["/sys/class/drm/card1"])
        .file(&format!("{dev}/vendor"), "0x1002\n")
        .file(&format!("{dev}/unique_id"), "5b7c23d1d3025b6c\n")
        .file(&format!("{dev}/product_name"), "AMD Radeon Test\n")
        .file(&format!("{dev}/mem_info_vram_total"), "17095983104\n");
    let cards = collect(&host, &[]);
    assert_eq!(cards.len(), 3);
    assert_eq!(cards[2].index, 2);
    assert_eq!(cards[2].uuid, "AMD-5b7c23d1d3025b6c");
    assert_eq!(cards[2].name, "AMD Radeon Test");
    // A previous AMD reading never stands in for an NVIDIA card or the reverse.
    let again = collect(&host, &cards);
    assert_eq!(again, cards);
}

#[test]
fn memory_is_laid_out_as_the_hardware_has_it() {
    // Cards with memory of their own: a pool apart from RAM.
    assert_eq!(
        memory(&parse(TWO)),
        Memory::Dedicated {
            used: 23024 * MIB,
            total: 49128 * MIB
        }
    );
    // An integrated GPU (DGX Spark, Jetson) answers and reports no memory
    // of its own: it shares RAM.
    let spark = parse("0, GPU-s, NVIDIA GB10, 42, [N/A], [N/A], 51\n");
    assert_eq!(spark.len(), 1);
    assert_eq!(memory(&spark), Memory::Shared);
    // A failed read answers nothing: unknown, not shared.
    let failed: Vec<Device> = parse(TWO).iter().map(Device::unavailable).collect();
    assert_eq!(memory(&failed), Memory::Unknown);
    assert_eq!(memory(&[]), Memory::Unknown);
    // One card of two without a total: no sum to draw, and not shared.
    let mixed = parse(&format!(
        "{TWO}2, GPU-c, NVIDIA test, 5, [N/A], [N/A], 40\n"
    ));
    assert_eq!(memory(&mixed), Memory::Unknown);
}
