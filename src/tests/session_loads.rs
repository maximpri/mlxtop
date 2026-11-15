// SPDX-License-Identifier: MIT
//! Peak memory per model load, kept after the model is unloaded.
use super::*;

const GIB: u64 = 1024 * 1024 * 1024;

/// The runtime serving `model` with `rss` in memory and requests holding
/// `contexts` tokens.
fn serving(model: &str, rss: u64, contexts: &[u64]) -> Sample {
    Sample {
        llm_provider: "oMLX".into(),
        llm_model: model.into(),
        llm_source: crate::domain::TelemetrySource::Live,
        llm_count: 2,
        llm_rss: rss,
        llm_requests: contexts
            .iter()
            .map(|context| RequestUsage {
                prompt: *context,
                output: Some(0),
                ..RequestUsage::default()
            })
            .collect(),
        ..Sample::default()
    }
}

#[test]
fn each_model_load_keeps_its_peak_after_the_model_is_gone() {
    let mut session = Session::default();
    session.observe(&serving("Qwen3.6-27B-4bit", 15 * GIB, &[]));
    session.observe(&serving("Qwen3.6-27B-4bit", 21 * GIB, &[60_000, 4_000]));
    session.observe(&serving("Qwen3.6-27B-4bit", 18 * GIB, &[70_000]));
    // Another model: the first load is kept as it peaked.
    session.observe(&serving("Qwen3.8-27B-oQ4e-mtp", 26 * GIB, &[1_900]));
    assert_eq!(
        session
            .loads
            .loads
            .iter()
            .map(|load| (load.model.as_str(), load.peak, load.requests, load.context))
            .collect::<Vec<_>>(),
        [
            ("Qwen3.6-27B-4bit", 21 * GIB, 2, Some(64_000)),
            ("Qwen3.8-27B-oQ4e-mtp", 26 * GIB, 1, Some(1_900)),
        ]
    );
    assert_eq!(
        load_lines(&session.loads),
        [
            "Peak memory per model load:",
            "  Qwen3.6-27B-4bit: 21.0 GiB with 64.0k tokens of context in 2 requests",
            "  Qwen3.8-27B-oQ4e-mtp: 26.0 GiB with 1.9k tokens of context · now",
        ]
    );
    // A sample without the runtime's telemetry names no model: it neither
    // ends the load nor counts toward its peak.
    let mut blind = serving("omlx-server", 40 * GIB, &[]);
    blind.llm_source = crate::domain::TelemetrySource::None;
    session.observe(&blind);
    assert_eq!(session.loads.loads.len(), 2);
    assert!(session.loads.loaded);
    // The runtime goes away: its model is unloaded, and stays listed.
    let gone = Sample {
        llm_count: 0,
        ..Sample::default()
    };
    session.observe(&gone);
    assert!(!session.loads.loaded);
    assert!(!load_lines(&session.loads)[2].ends_with("now"));
    // The same model loaded again is a load of its own.
    session.observe(&serving("Qwen3.8-27B-oQ4e-mtp", 24 * GIB, &[]));
    assert_eq!(session.loads.loads.len(), 3);
    assert_eq!(
        load_lines(&session.loads)[3],
        "  Qwen3.8-27B-oQ4e-mtp: 24.0 GiB · now"
    );
}

#[test]
fn the_summary_lists_loads_whose_memory_was_read_and_keeps_the_last_few() {
    let mut session = Session::default();
    // A runtime whose memory is not read, and a remote one, give nothing.
    session.observe(&serving("tiny", 0, &[]));
    let mut remote = serving("far", 9 * GIB, &[]);
    remote.llm_remote = true;
    session.observe(&remote);
    assert_eq!(session.loads.loads.len(), 1);
    assert!(load_lines(&session.loads).is_empty());
    let summary = lines(&session, &History::default(), &Sample::default(), None);
    assert!(!summary.iter().any(|line| line.starts_with("Peak memory")));
    // Twelve switches between two models: the last eight loads stay.
    for index in 0..12 {
        let model = if index % 2 == 0 { "a" } else { "b" };
        session.observe(&serving(model, (index + 1) * GIB, &[]));
    }
    assert_eq!(session.loads.loads.len(), LOADS_KEPT);
    assert_eq!(session.loads.loads.front().unwrap().peak, 5 * GIB);
    let summary = lines(&session, &History::default(), &Sample::default(), None);
    let at = summary
        .iter()
        .position(|line| line == "Peak memory per model load:")
        .expect("listed in the summary");
    assert_eq!(summary.len(), at + 1 + LOADS_KEPT);
    assert_eq!(summary.last().unwrap(), "  b: 12.0 GiB · now");
}
