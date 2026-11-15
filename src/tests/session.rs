// SPDX-License-Identifier: MIT
use super::*;
use crate::domain::{CorrelationInsight, DropEpisode, RequestUsage, ThroughputDirection};
use std::time::{Duration, SystemTime};

fn at(second: u64) -> Option<SystemTime> {
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000 + second))
}

fn writing(second: u64, tps: f64) -> Sample {
    Sample {
        llm_observed_at: at(second),
        llm_generation_tps: Some(tps),
        llm_generation_tps_live: true,
        system_power_mw: Some(60_000),
        ..Sample::default()
    }
}

#[test]
fn a_session_counts_work_energy_and_the_speed_each_cause_cost() {
    let mut session = Session::default();
    for second in 0..10 {
        session.observe(&writing(second, 50.0));
    }
    // Four seconds at half the usual speed, with paging beside it.
    for second in 10..14 {
        let mut sample = writing(second, 25.0);
        sample.correlation = CorrelationInsight {
            direction: ThroughputDirection::Down,
            cause: CorrelationCause::Paging,
            episode: Some(DropEpisode {
                reference: 50.0,
                ..Default::default()
            }),
            ..CorrelationInsight::default()
        };
        session.observe(&sample);
    }
    // A stall costs its whole interval.
    let mut stalled = writing(14, 0.0);
    stalled.llm_generation_tps_live = false;
    stalled.correlation = CorrelationInsight {
        direction: ThroughputDirection::Down,
        cause: CorrelationCause::Stall,
        ..CorrelationInsight::default()
    };
    session.observe(&stalled);
    // A gap longer than polling is not counted.
    session.observe(&writing(100, 50.0));

    assert_eq!(session.generating, 14.0);
    assert_eq!(session.tokens, 9.0 * 50.0 + 4.0 * 25.0);
    assert_eq!(session.lost.get("paging"), Some(&2.0));
    assert_eq!(session.lost.get("generation stalled"), Some(&1.0));
    assert_eq!(session.joules, 14.0 * 60.0);
    let per_token = session.joules_per_token().unwrap();
    assert!((per_token - 840.0 / 550.0).abs() < 1e-9);

    let mut requests = History::default();
    let request = |id: &str, prompt, output, queued, prefilled: Option<u64>, second| RequestUsage {
        id: id.into(),
        prompt,
        output: Some(output),
        queued,
        prefilled,
        observed_at: at(second),
        ..RequestUsage::default()
    };
    requests.observe(&[request("a", 30_000, 0, true, None, 0)]);
    requests.observe(&[request("a", 30_000, 0, false, Some(1000), 2)]);
    requests.observe(&[request("a", 30_000, 10, false, None, 12)]);
    requests.observe(&[request("a", 30_000, 400, false, None, 22)]);
    let timing = requests
        .timing(&request("a", 0, 0, false, None, 0))
        .unwrap();
    assert_eq!(
        (timing.queued, timing.prefill, timing.decode),
        (2.0, 10.0, 10.0)
    );
    assert_eq!(timing.first_token(), 12.0);
    assert_eq!(
        crate::collector::timing_label(&timing),
        "queued 2.0s → prompt 10s → answer 10s"
    );

    let sample = Sample {
        llm_cache_efficiency: Some(91.0),
        ..Sample::default()
    };
    let text = lines(&session, &requests, &sample, Some(300.0)).join("\n");
    for expected in [
        "Watched 1m 40s · writing 14s",
        "Speed lost: 3s of work",
        "paging",
        "generation stalled",
        "Requests: 1 · time went to queue 9% · prompt 45% · answer 45%",
        "first token: median 12s · slowest 12s",
        "slowest: a · 40 tok/s · 30.0k prompt",
        "KV cache: 91% of prompt tokens served from cache since the runtime started · ≈5m 0s of prefill saved while watched",
        "Energy: 1.53 J per token written",
    ] {
        assert!(text.contains(expected), "missing {expected}\n{text}");
    }
    // The biggest cost comes first.
    assert!(text.find("paging").unwrap() < text.find("generation stalled").unwrap());

    let quiet = lines(
        &Session::default(),
        &History::default(),
        &Sample::default(),
        None,
    );
    assert!(quiet
        .iter()
        .any(|line| line.contains("no slowdown or stall")));
}

#[test]
fn energy_counts_the_gpus_own_power_where_the_machine_reports_none() {
    use crate::gpu::{Device, Limits, Vendor};
    let card = |uuid: &str, milliwatts: Option<u64>| Device {
        index: 0,
        uuid: uuid.into(),
        vendor: Vendor::Nvidia,
        name: "NVIDIA RTX 4090".into(),
        utilization: Some(97),
        used: Some(1 << 30),
        total: Some(24 << 30),
        temperature: Some(70),
        throttle_reasons: None,
        unified: false,
        limits: Limits {
            power_mw: milliwatts,
            ..Limits::default()
        },
    };
    let on_cards = |gpus: Vec<Device>, system: Option<u64>| {
        let mut session = Session::default();
        for second in 0..10 {
            let mut sample = writing(second, 50.0);
            sample.system_power_mw = system;
            sample.gpus = gpus.clone();
            session.observe(&sample);
        }
        session
    };
    // Linux and Windows: the cards' own power, every card's.
    let cards = vec![card("GPU-a", Some(180_000)), card("GPU-b", Some(20_000))];
    let session = on_cards(cards.clone(), None);
    assert_eq!(session.joules, 9.0 * 200.0);
    assert!(session.gpu_power);
    assert_eq!(session.joules_per_token(), Some(1800.0 / 450.0));
    let sample = Sample {
        gpus: cards.clone(),
        ..Sample::default()
    };
    let text = lines(&session, &History::default(), &sample, None).join("\n");
    assert!(
        text.contains("Energy: 4.00 J per token written · 0.5 Wh while working · GPU power only"),
        "{text}"
    );
    // An APU's reading covers its CPU cores too.
    let mut apu = card("AMD-apu", Some(86_000));
    apu.vendor = Vendor::Amd;
    apu.unified = true;
    let apu_session = on_cards(vec![apu.clone()], None);
    let text = lines(
        &apu_session,
        &History::default(),
        &Sample {
            gpus: vec![apu],
            ..Sample::default()
        },
        None,
    )
    .join("\n");
    assert!(text.contains("· APU power only"), "{text}");
    // The whole machine, where the OS reports it, wins.
    let session = on_cards(cards, Some(60_000));
    assert_eq!(session.joules, 9.0 * 60.0);
    assert!(!session.gpu_power);
    let text = lines(&session, &History::default(), &Sample::default(), None).join("\n");
    assert!(
        text.contains("Wh while working\n") || text.ends_with("Wh while working"),
        "{text}"
    );
    // One card without a reading: unknown, not the other card's share.
    let session = on_cards(
        vec![card("GPU-a", Some(180_000)), card("GPU-b", None)],
        None,
    );
    assert_eq!(session.joules, 0.0);
    assert_eq!(session.joules_per_token(), None);
}
