// SPDX-License-Identifier: MIT
use super::*;

fn usual(values: impl IntoIterator<Item = f64>) -> Usual {
    let mut usual = Usual::default();
    for value in values {
        usual.learn(value);
    }
    usual
}

#[test]
fn a_signal_has_no_usual_value_until_it_has_been_seen_enough() {
    let mut signal = usual([50.0; LEARN_SAMPLES - 1]);
    assert_eq!(signal.level(), None);
    assert_eq!(signal.spread(), 0.0);
    assert_eq!(signal.below(1.0, 0.0, 0.0), None, "nothing to compare with");
    assert_eq!(signal.above(999.0, 0.0, 0.0), None);
    signal.learn(50.0);
    assert_eq!(signal.level(), Some(50.0));
    // A reading that is not a number teaches nothing.
    signal.learn(f64::NAN);
    signal.learn(f64::INFINITY);
    assert_eq!(signal.level(), Some(50.0));
    assert_eq!(median(Vec::new()), 0.0);
}

#[test]
fn the_usual_value_is_the_middle_and_the_spread_ignores_a_burst() {
    // Eight steady readings and one burst: the middle and the spread are
    // those of the steady ones.
    let signal = usual([49.0, 50.0, 51.0, 50.0, 49.0, 51.0, 50.0, 50.0, 500.0]);
    assert_eq!(signal.level(), Some(50.0));
    assert!(
        (signal.spread() - 1.4826).abs() < 1e-9,
        "{}",
        signal.spread()
    );
    // An even count takes the mean of the two middle readings.
    assert_eq!(
        usual([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]).level(),
        Some(4.5)
    );
}

#[test]
fn a_steady_signal_shows_a_small_change_and_a_jumpy_one_does_not() {
    // 50 ± 1: forty is well outside what it ordinarily does.
    let steady = usual((0..40).map(|index| 49.0 + (index % 3) as f64));
    assert_eq!(steady.below(40.0, 0.0, 0.0), Some(50.0));
    assert_eq!(steady.below(47.0, 0.0, 0.0), None, "within 3.5 spreads");
    // The same signal swinging between 20 and 120: forty is ordinary, and
    // so is everything down to zero.
    let jumpy = usual((0..40).map(|index| if index % 2 == 0 { 20.0 } else { 120.0 }));
    assert_eq!(jumpy.level(), Some(70.0));
    assert_eq!(jumpy.below(40.0, 0.0, 0.0), None);
    assert_eq!(jumpy.below(0.0, 0.0, 0.0), None);
    assert_eq!(jumpy.above(130.0, 0.0, 0.0), None);
}

#[test]
fn a_change_too_small_to_matter_is_not_unusual_however_steady_the_signal() {
    // A signal that never moves has no spread: any change clears it.
    let flat = usual([50.0; 20]);
    assert_eq!(flat.spread(), 0.0);
    assert_eq!(flat.below(49.9, 0.0, 0.0), Some(50.0));
    // A share of the usual value, and an outright least, set a floor.
    assert_eq!(flat.below(41.0, 0.20, 0.0), None, "less than a fifth");
    assert_eq!(flat.below(40.0, 0.20, 0.0), Some(50.0));
    assert_eq!(flat.above(51.0, 0.0, 2.0), None, "less than two");
    assert_eq!(flat.above(52.0, 0.0, 2.0), Some(50.0));
    assert_eq!(flat.range(0.20, 2.0), Some((50.0, 10.0)));
    // Above and below are different sides.
    assert_eq!(flat.above(40.0, 0.20, 0.0), None);
    assert_eq!(flat.below(60.0, 0.20, 0.0), None);
}

#[test]
fn old_samples_leave_and_a_signal_can_start_over() {
    let mut signal = usual([10.0; WINDOW]);
    for _ in 0..WINDOW {
        signal.learn(90.0);
    }
    assert_eq!(signal.level(), Some(90.0), "the window holds the newest");
    signal.relearn([30.0; LEARN_SAMPLES]);
    assert_eq!(signal.level(), Some(30.0));
    signal.relearn([]);
    assert_eq!(signal.level(), None);
}
