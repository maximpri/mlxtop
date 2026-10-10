// SPDX-License-Identifier: MIT
use super::*;

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
    let core = "nvidia-smi --query-gpu=index,uuid,name,utilization.gpu,memory.used,memory.total,temperature.gpu --format=csv,noheader,nounits";
    let host = FakeHost::default().command(core, TWO).command(
        "nvidia-smi --query-gpu=uuid,clocks_event_reasons.active --format=csv,noheader",
        "GPU-b, 0x0000000000000040\n",
    );
    let cards = collect(&host, &[]);
    assert_eq!(cards[1].throttle(), Some(Throttle::Thermal));
    assert_eq!(cards[0].throttle_reasons, None);
    let calls = host.calls();
    assert_eq!(calls.len(), 3, "{calls:?}");
    let empty = FakeHost::default();
    assert!(collect(&empty, &[]).is_empty());
    assert_eq!(empty.calls().len(), 1);
}
