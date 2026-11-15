// SPDX-License-Identifier: MIT
//! "The KV cache was full, so requests were recomputed": a slowdown's
//! reason when the runtime's count of preempted (recomputed) requests rises
//! during it, after the host's memory limits and before the disk work and
//! prefill a recomputed request causes.
use super::*;

const KV_FULL: &str = "The KV cache was full, so requests were recomputed: give the runtime more KV memory, or run fewer requests at once.";

/// A poll at `tps` with the runtime's preemption count at `total`.
fn counted(second: u64, tps: f64, total: Option<u64>) -> Sample {
    let mut sample = paging(second, tps, 0);
    sample.llm_preemptions_total = total;
    sample
}

/// Twelve polls at 50 tok/s with the count steady, then `rest`.
fn after_steady(rest: impl IntoIterator<Item = Sample>) -> Vec<Sample> {
    let mut samples: Vec<Sample> = (0..12)
        .map(|second| counted(second, 50.0, Some(5)))
        .collect();
    samples.extend(rest);
    observe_all(&mut CorrelationEngine::default(), &mut samples);
    samples
}

#[test]
fn a_slowdown_while_requests_are_recomputed_is_put_down_to_a_full_kv_cache() {
    // Two requests recomputed each second while speed halves.
    let samples =
        after_steady((12..18).map(|second| counted(second, 25.0, Some(5 + 2 * (second - 11)))));
    let slowed = &samples[14].correlation;
    assert!(slowed.is_material_drop(), "{}", slowed.summary);
    assert_eq!(slowed.cause, CorrelationCause::KvPreemption);
    assert_eq!(
        slowed.lead,
        "KV cache full · 2 requests recomputed (usual 0)"
    );
    assert_eq!(
        slowed.summary,
        "GEN ↓50.0% (50.0→25.0 tok/s) · unusual: KV cache full · 2 requests recomputed (usual 0)"
    );
    assert_eq!(crate::diagnosis::slowdown_next_step(&samples[14]), KV_FULL);
    assert_eq!(
        journal_rows(&samples, &["SLOWDOWN"]),
        vec![(
            "SLOWDOWN".to_owned(),
            format!(
                "GEN ↓50.0% (50.0→25.0 tok/s) · KV cache full · 2 requests recomputed (usual 0) → {KV_FULL}"
            )
        )]
    );
    // Its reading has no chart of its own: nothing but generation is marked.
    assert_eq!(crate::drop_marks::linked(slowed), None);
    assert_eq!(slowed.cause.label(), "KV cache full");
    assert_eq!(crate::export::cause_name(slowed.cause), "kv_preemption");
    // One request is said as one.
    let one = after_steady((12..18).map(|second| counted(second, 25.0, Some(5 + second - 11))));
    assert_eq!(
        one[14].correlation.lead,
        "KV cache full · 1 request recomputed (usual 0)"
    );
}

#[test]
fn a_full_kv_cache_comes_after_paging_and_before_the_prefill_it_causes() {
    // A recomputed request reads its prompt again beside the answer.
    let samples = after_steady((12..18).map(|second| {
        beside(
            counted(second, 25.0, Some(5 + 2 * (second - 11))),
            30_000,
            true,
        )
    }));
    let slowed = &samples[14].correlation;
    assert_eq!(slowed.cause, CorrelationCause::KvPreemption);
    assert_eq!(
        slowed.unusual,
        vec![
            CorrelationCause::KvPreemption,
            CorrelationCause::ConcurrentPrefill
        ]
    );
    // Paging beside it is the host's limit, named first.
    let samples = after_steady((12..18).map(|second| {
        let mut sample = counted(second, 25.0, Some(5 + 2 * (second - 11)));
        sample.swap_in = 96 * MIB;
        sample
    }));
    let slowed = &samples[14].correlation;
    assert_eq!(slowed.cause, CorrelationCause::Paging);
    assert_eq!(
        slowed.unusual,
        vec![CorrelationCause::Paging, CorrelationCause::KvPreemption]
    );
    let order: Vec<Signal> = SIGNALS.to_vec();
    let at = |signal| order.iter().position(|known| *known == signal).unwrap();
    assert!(at(Signal::Pressure) < at(Signal::KvPreemption));
    assert!(at(Signal::Paging) < at(Signal::KvPreemption));
    assert!(at(Signal::KvPreemption) < at(Signal::Cpu));
    assert!(at(Signal::KvPreemption) < at(Signal::GpuIdle));
}

#[test]
fn no_rise_in_the_count_is_no_full_kv_cache() {
    // Steady, unknown, or gone back from 5 because the runtime restarted.
    for (name, totals) in [
        ("steady", [Some(5); 6]),
        ("unknown", [None; 6]),
        ("restarted", [Some(0); 6]),
    ] {
        let samples = after_steady(
            (12..18)
                .zip(totals)
                .map(|(second, total)| counted(second, 25.0, total)),
        );
        let slowed = &samples[14].correlation;
        assert!(slowed.is_material_drop(), "{name}");
        assert_eq!(slowed.cause, CorrelationCause::None, "{name}");
        assert!(!slowed.unusual.contains(&CorrelationCause::KvPreemption));
    }
    // Recomputed while speed holds: said in Diagnostics, never painted.
    let samples =
        after_steady((12..15).map(|second| counted(second, 50.0, Some(5 + 2 * (second - 11)))));
    let held = &samples[13].correlation;
    assert!(!held.slow && !held.is_material_drop());
    assert_eq!(held.unusual, vec![CorrelationCause::KvPreemption]);
    assert_eq!(
        held.notes,
        vec!["KV cache full · 2 requests recomputed (usual 0)"]
    );
    assert_eq!(held.impact(CorrelationCause::KvPreemption), None);
}

#[test]
fn a_stall_beside_a_recomputed_request_names_the_full_cache() {
    let mut samples: Vec<Sample> = (0..12)
        .map(|second| {
            let mut sample = decoding(second, 50.0, 500 + second * 40, 0.05, 99);
            sample.llm_preemptions_total = Some(5);
            sample
        })
        .collect();
    let mut stalled = decoding(12, 0.0, 980, 3.0, 99);
    stalled.llm_preemptions_total = Some(6);
    samples.push(stalled);
    observe_all(&mut CorrelationEngine::default(), &mut samples);
    let stall = &samples[12].correlation;
    assert_eq!(stall.cause, CorrelationCause::Stall);
    assert_eq!(
        stall.lead,
        "no new token for 3s at 980 out · GPU 99% · KV cache full, 1 request recomputed"
    );
    assert_eq!(stall.unusual, vec![CorrelationCause::KvPreemption]);
    assert_eq!(crate::diagnosis::slowdown_next_step(&samples[12]), KV_FULL);
}

#[test]
fn the_tally_counts_a_full_kv_cache_under_its_own_reason() {
    let mut samples: Vec<Sample> = (0..12)
        .map(|second| counted(second, 50.0, Some(5)))
        .collect();
    samples.extend((12..18).map(|second| counted(second, 25.0, Some(5 + 2 * (second - 11)))));
    samples.extend((18..24).map(|second| counted(second, 50.0, Some(19))));
    let replay = crate::recording::replay(&mut samples, defaults());
    assert_eq!(replay.tally.len(), 1);
    let (cause, tally) = replay.tally[0];
    assert_eq!(cause, CorrelationCause::KvPreemption);
    assert_eq!((tally.samples, tally.episodes), (6, 1));
}
