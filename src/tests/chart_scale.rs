// SPDX-License-Identifier: MIT
use super::*;

#[test]
fn ceilings_round_up_to_a_readable_step_and_never_reach_zero() {
    assert_eq!(ceiling(837, 1), 1000);
    assert_eq!(ceiling(0, 1), 1);
    // Rate charts stand on zero: the visible peak sets the top.
    assert_eq!(ceiling(360, 10), 400);
    assert!(ceiling(u64::MAX / 2, 1) >= u64::MAX / 2);
}

#[test]
fn byte_rate_ceilings_are_round_in_the_unit_they_are_shown_in() {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    // The 1 MiB/s paging guide tops out at 1.5 MiB/s, not 1.4.
    assert_eq!(ceiling_bytes(MIB, 1), 3 * MIB / 2);
    assert_eq!(crate::formatting::bytes(ceiling_bytes(MIB, 1)), "1.5 MiB");
    for peak in [
        1,
        900,
        16_000,
        205 * KIB,
        70 * MIB,
        300 * MIB,
        3 * 1024 * MIB,
    ] {
        let top = ceiling_bytes(peak, 1);
        assert!(top >= peak, "{peak} under {top}");
        let unit = [1024 * MIB, MIB, KIB, 1]
            .into_iter()
            .find(|unit| top >= *unit)
            .unwrap();
        // A whole number of tenths of its unit: the label loses nothing.
        assert_eq!(top * 10 % unit, 0, "{peak} tops at {top}");
    }
    assert_eq!(ceiling_bytes(0, 1), 1);
    assert!(ceiling_bytes(u64::MAX / 2, 1) >= u64::MAX / 2);
}
