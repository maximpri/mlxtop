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
