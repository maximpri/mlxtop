// SPDX-License-Identifier: MIT
//! The wired-limit trigger: GPU memory at 90% of the kernel's GPU wired
//! limit or of Metal's recommended working set, where mlx-lm warns that
//! generation can be slow.
use super::*;

const GIB: u64 = 1024 * MIB;

fn holding(gpu: Option<u64>, wired: u64, kernel: Option<u64>, working_set: Option<u64>) -> Sample {
    let mut sample = busy_sample();
    sample.gpu_in_use = gpu;
    sample.wired = wired;
    sample.metal.resource_limit = kernel;
    sample.mlx.recommended_working_set = working_set;
    sample
}

#[test]
fn the_advice_starts_at_90_percent_of_the_kernels_limit() {
    let limit = 32 * GIB;
    let ninety = (limit * 9).div_ceil(10);
    // Metal's count is held against the limit; wired RAM only stands in.
    let at = holding(Some(ninety), 40 * GIB, Some(limit), None);
    assert_eq!(
        wired_limit_step(&at).as_deref(),
        Some("GPU memory is 28.8 GiB of the 32.0 GiB GPU limit, where MLX warns that generation can be slow: raise it (sudo sysctl iogpu.wired_limit_mb=…) or shorten the context.")
    );
    let below = holding(Some(ninety - 1), 40 * GIB, Some(limit), None);
    assert_eq!(wired_limit_step(&below), None);
    let wired = holding(None, 31 * GIB, Some(limit), None);
    assert!(wired_limit_step(&wired)
        .unwrap()
        .starts_with("Wired memory is 31.0 GiB of the 32.0 GiB GPU limit"));
    // No limit known, or one read as zero: nothing to say.
    assert_eq!(
        wired_limit_step(&holding(Some(31 * GIB), 31 * GIB, None, None)),
        None
    );
    assert_eq!(
        wired_limit_step(&holding(Some(31 * GIB), 31 * GIB, Some(0), Some(0))),
        None
    );
}

#[test]
fn metals_recommended_working_set_triggers_it_too_and_the_nearer_limit_is_named() {
    // Without `iogpu.wired_limit_mb`, Metal's recommended working set: what
    // MLX wires up to.
    let set = holding(Some(25 * GIB), 26 * GIB, None, Some(27 * GIB));
    assert_eq!(
        wired_limit_step(&set).as_deref(),
        Some("GPU memory is 25.0 GiB of Metal's 27.0 GiB recommended working set, where MLX warns that generation can be slow: raise iogpu.wired_limit_mb (sudo sysctl iogpu.wired_limit_mb=…) or shorten the context.")
    );
    // Both known: 90% of either is enough, and the nearer one is named.
    let both = holding(Some(25 * GIB), 26 * GIB, Some(32 * GIB), Some(27 * GIB));
    assert!(wired_limit_step(&both)
        .unwrap()
        .contains("of Metal's 27.0 GiB recommended working set"));
    let equal = holding(Some(30 * GIB), 31 * GIB, Some(32 * GIB), Some(32 * GIB));
    assert!(wired_limit_step(&equal)
        .unwrap()
        .contains("of the 32.0 GiB GPU limit"));
    assert_eq!(
        wired_limit_step(&holding(
            Some(24 * GIB),
            26 * GIB,
            Some(32 * GIB),
            Some(27 * GIB)
        )),
        None
    );
}

#[test]
fn a_slowdown_blamed_on_gpu_memory_takes_the_advice_from_the_trigger() {
    let mut sample = holding(Some(25 * GIB), 26 * GIB, None, Some(27 * GIB));
    for cause in [CorrelationCause::MetalMemory, CorrelationCause::ModelMemory] {
        sample.correlation.cause = cause;
        assert_eq!(
            slowdown_next_step(&sample),
            wired_limit_step(&sample).unwrap()
        );
    }
    // Below the trigger the advice stays the model's own.
    sample.gpu_in_use = Some(12 * GIB);
    assert!(slowdown_next_step(&sample).starts_with("The model's memory is near its ceiling"));
    // Paging is not a wired limit: its advice does not change.
    sample.gpu_in_use = Some(26 * GIB);
    sample.correlation.cause = CorrelationCause::Paging;
    assert!(slowdown_next_step(&sample).starts_with("The model no longer fits in RAM"));
}

#[test]
fn diagnostics_give_the_advice_once() {
    let mut near = holding(Some(29 * GIB), 30 * GIB, Some(32 * GIB), None);
    let step = wired_limit_step(&near).unwrap();
    let said = |sample: &Sample| {
        crate::report::diagnostic_lines(sample, None)
            .iter()
            .filter(|line| line.contains(&step))
            .count()
    };
    // Healthy generation near the limit: in words, in Diagnostics only.
    near.correlation.direction = ThroughputDirection::Flat;
    assert_eq!(assess(&near).next, "");
    assert_eq!(wired_limit_note(&near, ""), Some(step.clone()));
    assert_eq!(said(&near), 1);
    // A slowdown blamed on GPU memory already says it as its next step.
    near.correlation = CorrelationInsight {
        direction: ThroughputDirection::Down,
        cause: CorrelationCause::MetalMemory,
        slow: true,
        ..CorrelationInsight::default()
    };
    assert_eq!(assess(&near).next, step);
    assert_eq!(wired_limit_note(&near, &step), None);
    assert_eq!(said(&near), 1);
}
