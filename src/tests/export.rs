// SPDX-License-Identifier: MIT
// The JSON snapshot and the stream: the schema against a golden file, the
// privacy promise, unknown readings as null, the estimates beside what the
// runtime counts, what only a session knows, and the stream's ending. The
// static report's text is pinned here too, from the same fixtures, so the
// JSON work cannot move a byte of `--once`.
use crate::domain::{
    CorrelationCause, CorrelationInsight, CorrelationKey, DiskActivity, DropEpisode, LlmProcess,
    MetalTelemetry, Phase, RequestUsage, Sample, SsdCache, TelemetrySource, ThermalLevel,
    ThermalState, ThroughputDirection, Tone, MIB,
};
use crate::host::Platform;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

const GIB: u64 = 1024 * MIB;
/// 2025-11-15 07:03:27.123 UTC: when the fixtures were taken.
const TS: u64 = 1_763_190_207_123;

fn at(epoch_ms: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(epoch_ms)
}

/// A process row as the Processes view lists it: its name and command line
/// must never reach the JSON.
fn private_process() -> LlmProcess {
    LlmProcess {
        pid: 4242,
        name: "omlx-worker".into(),
        command: "/Users/someone/.omlx/bin/omlx serve --api-key sk-private-key".into(),
        rss: 17 * GIB,
        cpu: 45.0,
        memory_percent: Some(47.2),
        state: "R".into(),
        pageins: Some(12),
        pagein_rate: Some(0.0),
        footprint: Some(19 * GIB),
    }
}

/// What WP3 works out for the owner's Qwen3.8-27B-oQ4e-mtp served through
/// Splash with int8 KV (see tests/model_facts.rs): 16 full-attention layers
/// of 4 KV heads of 256 keep keys and values, at a byte a value and a byte
/// of scale per 64.
pub(crate) fn owner_model_facts() -> crate::model_facts::ModelFacts {
    crate::model_facts::ModelFacts {
        bytes_per_pass: Some(15_020_479_488),
        parameters: Some(26_896_000_000),
        bits_per_weight: Some(4.6913),
        quantization: Some("4/5-bit g64".into()),
        kv_layers: Some(16),
        kv_heads: Some(4),
        head_dim: Some(256),
        kv_dtype: Some("int8".into()),
        kv_bytes_per_element: Some(1.0 + 1.0 / 64.0),
        fixed_state_bytes: Some(154_140_672 + 5_898_240),
        context_configured: Some(131_072),
        context_trained: Some(262_144),
        draft: Some(true),
        draft_bytes: Some(3_848_808_960),
        runtime_version: Some("oMLX 0.7.3 · Splash 1.5.0".into()),
        ..crate::model_facts::ModelFacts::default()
    }
}

/// The owner's Mac Studio (M4 Max, 32-core GPU, 36 GiB) serving a 27B model
/// through oMLX and Splash, twelve seconds into a slowdown that coincides
/// with paging.
pub(crate) fn mac_studio_slowdown() -> Sample {
    Sample {
        updated: "02:03:27".into(),
        sequence: 42,
        utc_offset: Some(-18_000),
        pressure: "GREEN".into(),
        pressure_meaning: "normal".into(),
        pressure_tone: Tone::Green,
        availability: Some(24),
        resident_memory: Some(32 * GIB),
        total_memory: 36 * GIB,
        wired: 20 * GIB,
        compressor: GIB,
        compressed_logical: 5 * GIB / 2,
        anonymous: 6 * GIB,
        file_backed: 5 * GIB,
        swap_total: 2 * GIB,
        swap_used: 3 * GIB / 2,
        swap_available: true,
        paging_unavailable: false,
        swap_in: 6 * MIB,
        swap_out: 2 * MIB,
        page_in: 512 * 1024,
        page_out: 0,
        swap_growth: 2 * MIB as i64,
        compress: 3 * MIB,
        decompress: MIB,
        reactivated: 0,
        vm_available: true,
        gpu_util: Some(96),
        gpu_alloc: Some(19 * GIB),
        gpu_in_use: Some(18 * GIB),
        metal: MetalTelemetry {
            device_name: Some("Apple M4 Max".into()),
            architecture: Some("arm64".into()),
            gpu_cores: Some(32),
            renderer_util: Some(96),
            tiler_util: Some(41),
            resource_limit: Some(28_000 * MIB),
            machine_model: None,
        },
        thermal: "nominal".into(),
        thermal_state: ThermalState {
            level: Some(ThermalLevel::Nominal),
            cpu_speed_limit: None,
        },
        system_power_mw: Some(35_712),
        model: owner_model_facts(),
        llm_count: 1,
        llm_rss: 17 * GIB,
        process_memory: Some(crate::process_memory::Reading {
            pid: 4242,
            started: 1,
            resident: 2 * GIB,
            footprint: 19 * GIB,
            peak: 20 * GIB,
            disk_read: 0,
            disk_written: 0,
            at: Instant::now(),
        }),
        process_memory_growth: Some(MIB as i64),
        disk: DiskActivity {
            read: Some(0),
            write: Some(64 * 1024),
            ..DiskActivity::default()
        },
        ssd_cache: Some(SsdCache {
            used: 40 * GIB,
            capacity: 100 * GIB,
            ..SsdCache::default()
        }),
        llm_cpu: 45.0,
        llm_processes: vec![private_process()],
        largest_consumer: Some("Google Chrome Helper".into()),
        largest_other: Some(("Google Chrome".into(), 2 * GIB)),
        llm_model: "Qwen3.8-27B-oQ4e-mtp".into(),
        llm_provider: "oMLX".into(),
        llm_status: "generating".into(),
        llm_source: TelemetrySource::Live,
        llm_observed_at: Some(at(TS - 250)),
        llm_generation_tps: Some(61.8),
        llm_generation_tps_live: true,
        llm_prefill_tps: Some(298.4),
        llm_prefill_tps_live: false,
        llm_output_tokens: Some(812),
        llm_prompt_tokens: Some(40_211),
        llm_requests: vec![RequestUsage {
            provider: "oMLX".into(),
            model: "Qwen3.8-27B-oQ4e-mtp".into(),
            id: "req-7f3a".into(),
            prompt: 40_211,
            cached: Some(39_936),
            output: Some(812),
            output_tps: Some(61.8),
            completed: false,
            queued: false,
            prefilled: None,
            ttft_ms: Some(1_840),
            idle_for: None,
            observed_at: Some(at(TS - 250)),
        }],
        llm_cache_efficiency: Some(91.5),
        llm_total_prompt_tokens: Some(195_000_000),
        llm_total_cached_tokens: Some(178_425_000),
        llm_watched_cached_tokens: Some(1_250_000),
        llm_cache_interval_efficiency: Some(99.3),
        llm_prefix_hit_rate: None,
        llm_draft_acceptance: Some(0.78),
        llm_active_requests: Some(1),
        llm_waiting_requests: Some(0),
        llm_model_memory: Some(16 * GIB),
        llm_model_memory_max: Some(30 * GIB),
        llm_details: Some("context capacity 262144 tokens · server TTFT 1.8s".into()),
        llm_engine: Some("Splash".into()),
        llm_remote: false,
        correlation: CorrelationInsight {
            phase: Phase::Generation,
            direction: ThroughputDirection::Down,
            cause: CorrelationCause::Paging,
            summary: "GEN ↓49.8% (123.0→61.8 tok/s) · unusual: paging 8.0 MiB/s (usual none)"
                .into(),
            details: "paging · paging 8.0 MiB/s (usual none)".into(),
            lead: "paging 8.0 MiB/s (usual none)".into(),
            stall_seconds: None,
            event_key: Some(CorrelationKey {
                direction: ThroughputDirection::Down,
                cause: CorrelationCause::Paging,
            }),
            delta_percent: Some(-49.756),
            episode: Some(DropEpisode {
                reference: 123.0,
                low: 58.4,
                seconds: 12.0,
                samples: 12,
            }),
            ended: None,
            repeat: false,
            slow: true,
            unusual: vec![CorrelationCause::Paging],
            notes: vec!["paging 8.0 MiB/s (usual none)".into()],
            usual_speed: Some((123.0, 98.4)),
            usual_for: "1 request · 41.0k context".into(),
            earlier: None,
        },
        llm_top: "omlx-worker".into(),
        llm_pid: 4242,
        impact: "PAGING ACTIVE".into(),
        impact_tone: Tone::Yellow,
        health: Some(65),
        grade: "CONSTRAINED".into(),
        limiter: "active paging".into(),
        rate_ready: true,
        ts: Some(TS),
        ..Sample::default()
    }
}

/// The same machine with an answer that has stopped: no token for 4 s.
pub(crate) fn mac_studio_stall() -> Sample {
    let mut sample = mac_studio_slowdown();
    sample.swap_in = 0;
    sample.swap_out = 0;
    sample.swap_growth = 0;
    sample.gpu_util = Some(3);
    sample.impact = "LLM READY".into();
    sample.impact_tone = Tone::Green;
    sample.grade = "HEALTHY".into();
    sample.health = Some(100);
    sample.limiter = "none".into();
    sample.llm_generation_tps = Some(0.0);
    sample.llm_requests[0].idle_for = Some(Duration::from_secs(4));
    sample.correlation = CorrelationInsight {
        phase: Phase::Generation,
        direction: ThroughputDirection::Down,
        cause: CorrelationCause::Stall,
        summary: "GEN stalled · no new token for 4s at 812 out · GPU idle".into(),
        details: "generation stalled · no new token for 4s at 812 out · GPU idle".into(),
        lead: "no new token for 4s at 812 out · GPU idle".into(),
        stall_seconds: Some(4.0),
        event_key: Some(CorrelationKey {
            direction: ThroughputDirection::Down,
            cause: CorrelationCause::Stall,
        }),
        slow: true,
        usual_speed: Some((123.0, 98.4)),
        usual_for: "1 request · 41.0k context".into(),
        ..CorrelationInsight::default()
    };
    sample
}

/// The same machine with the model loaded and nothing asked of it.
pub(crate) fn mac_studio_idle() -> Sample {
    let mut sample = mac_studio_slowdown();
    sample.swap_in = 0;
    sample.swap_out = 0;
    sample.swap_growth = 0;
    sample.compress = 0;
    sample.decompress = 0;
    sample.gpu_util = Some(0);
    sample.impact = "LLM READY".into();
    sample.impact_tone = Tone::Green;
    sample.grade = "HEALTHY".into();
    sample.health = Some(100);
    sample.limiter = "none".into();
    sample.llm_status = "idle".into();
    sample.llm_generation_tps = Some(123.4);
    sample.llm_generation_tps_live = false;
    sample.llm_requests.clear();
    sample.llm_active_requests = Some(0);
    sample.llm_draft_acceptance = None;
    sample.correlation = CorrelationInsight {
        usual_speed: Some((123.0, 98.4)),
        usual_for: "1 request · 41.0k context".into(),
        ..CorrelationInsight::default()
    };
    sample
}

/// A Mac with no runtime found: memory readings, and nothing to judge.
pub(crate) fn mac_no_runtime() -> Sample {
    Sample {
        llm_provider: "none".into(),
        llm_status: "offline".into(),
        llm_model: "not detected".into(),
        llm_count: 0,
        llm_rss: 0,
        llm_processes: Vec::new(),
        llm_top: "none".into(),
        llm_pid: 0,
        process_memory: None,
        process_memory_growth: None,
        llm_source: TelemetrySource::None,
        llm_observed_at: None,
        llm_generation_tps: None,
        llm_generation_tps_live: false,
        llm_prefill_tps: None,
        llm_output_tokens: None,
        llm_prompt_tokens: None,
        llm_requests: Vec::new(),
        llm_cache_efficiency: None,
        llm_total_prompt_tokens: None,
        llm_total_cached_tokens: None,
        llm_watched_cached_tokens: None,
        llm_cache_interval_efficiency: None,
        llm_draft_acceptance: None,
        llm_active_requests: None,
        llm_waiting_requests: None,
        llm_model_memory: None,
        llm_model_memory_max: None,
        llm_details: None,
        llm_engine: None,
        model: crate::model_facts::ModelFacts::default(),
        ssd_cache: None,
        disk: DiskActivity::default(),
        correlation: CorrelationInsight::default(),
        impact: "IDLE".into(),
        impact_tone: Tone::Green,
        grade: "HEALTHY".into(),
        health: Some(100),
        limiter: "none".into(),
        swap_in: 0,
        swap_out: 0,
        swap_growth: 0,
        gpu_util: Some(2),
        ..mac_studio_slowdown()
    }
}

/// A runtime on another machine: its readings are not this host's.
pub(crate) fn mac_remote_runtime() -> Sample {
    let mut sample = mac_studio_idle();
    sample.llm_remote = true;
    sample.llm_provider = "vLLM".into();
    sample.llm_engine = None;
    sample.llm_model = "Qwen/Qwen3-32B".into();
    sample.llm_status = "generating".into();
    sample.llm_generation_tps = Some(41.0);
    sample.llm_generation_tps_live = true;
    sample.llm_waiting_requests = Some(2);
    sample.llm_active_requests = Some(4);
    sample.llm_details = Some("remote API · throughput: server token deltas".into());
    sample.model = crate::model_facts::ModelFacts::default();
    sample
}

/// Splash's memory plan for the owner's model (tests/fixtures/splash_status.json):
/// KV pages of 32 tokens at 33,280 bytes a token, 4,736 of them. The one
/// request in flight, 41,023 tokens deep, holds 1,282.
const SPLASH_PAGE_TOKENS: u64 = 32;
const SPLASH_FREE_PAGES: u64 = 4_736 - 1_282;

/// The same slowdown with Splash serving the model on its own, so mlxtop
/// reads what Splash counts of its work, which oMLX keeps private: 7.8
/// tokens a verify pass, the KV room its memory plan has left and the
/// requests it suspended for room. Every estimate is known for it, so the
/// golden files are of this sample.
pub(crate) fn splash_slowdown() -> Sample {
    let mut sample = mac_studio_slowdown();
    sample.llm_provider = "Splash".into();
    sample.llm_engine = None;
    sample.llm_details = Some("DFlash draft acceptance 78% · server TTFT 1.8s".into());
    sample.llm_requests[0].provider = "Splash".into();
    sample.llm_tokens_per_pass = Some(7.8);
    sample.llm_kv_free_tokens = Some(SPLASH_FREE_PAGES * SPLASH_PAGE_TOKENS);
    sample.llm_kv_free_bytes = Some(SPLASH_FREE_PAGES * SPLASH_PAGE_TOKENS * 33_280);
    sample.llm_preemptions_total = Some(2);
    sample
}

/// The same server writing the same answer at its usual 123 tok/s, with
/// nothing unusual on the machine.
pub(crate) fn splash_decoding() -> Sample {
    let mut sample = splash_slowdown();
    sample.swap_in = 0;
    sample.swap_out = 0;
    sample.swap_growth = 0;
    sample.page_in = 0;
    sample.impact = "LLM READY".into();
    sample.impact_tone = Tone::Green;
    sample.grade = "HEALTHY".into();
    sample.health = Some(100);
    sample.limiter = "none".into();
    sample.llm_generation_tps = Some(123.0);
    sample.llm_requests[0].output_tps = Some(123.0);
    sample.correlation = CorrelationInsight {
        direction: ThroughputDirection::Flat,
        usual_speed: Some((123.0, 98.4)),
        usual_for: "1 request · 41.0k context".into(),
        ..CorrelationInsight::default()
    };
    sample
}

/// What a session's decode record holds once it has watched the server
/// decode at its usual speed for twenty samples, the fewest that give a
/// best: 64% of 410 GB/s per verify pass.
pub(crate) fn watched() -> crate::efficiency::Record {
    let decoding = splash_decoding();
    let mut record = crate::efficiency::Record::default();
    for _ in 0..20 {
        record.observe(&decoding);
    }
    record
}

/// A collector on a recorded Mac whose session's decode record is
/// `decode`, and whose requests' first tokens lost time every way the
/// session summary counts: what `stream` and `serve` judge each sample
/// beside.
pub(crate) fn watching(decode: crate::efficiency::Record) -> crate::collector::Collector {
    let mut collector = crate::collector::Collector::with_host(
        60,
        crate::test_support::offline_config(),
        Box::new(crate::test_support::macos_host()),
        Platform::MacOs,
        None,
    );
    collector.session.decode = decode;
    collector.session.request_findings = crate::request_findings::tests::every_finding();
    collector
}

/// The owner's server a day after oMLX 0.7.3 became 0.8.0, at 40 tok/s
/// where earlier sessions ran this work at 58: judged against them alone,
/// with nothing else unusual, six samples into the slowdown that is.
pub(crate) fn below_earlier_sessions() -> Sample {
    let mut sample = mac_studio_idle();
    sample.llm_status = "generating".into();
    sample.llm_generation_tps = Some(40.0);
    sample.llm_generation_tps_live = true;
    sample.llm_requests = mac_studio_slowdown().llm_requests;
    sample.llm_requests[0].output_tps = Some(40.0);
    sample.llm_active_requests = Some(1);
    sample.correlation = CorrelationInsight {
        phase: Phase::Generation,
        direction: ThroughputDirection::Down,
        cause: CorrelationCause::EarlierSessions,
        summary: "GEN ↓31.0% (58.0→40.0 tok/s) · below earlier sessions".into(),
        lead: "below earlier sessions".into(),
        event_key: Some(CorrelationKey {
            direction: ThroughputDirection::Down,
            cause: CorrelationCause::EarlierSessions,
        }),
        delta_percent: Some(-31.03),
        episode: Some(DropEpisode {
            reference: 58.0,
            low: 39.5,
            seconds: 6.0,
            samples: 6,
        }),
        slow: true,
        usual_speed: Some((58.0, 46.4)),
        usual_for: "1 request · 41.0k context".into(),
        earlier: Some(crate::baseline_store::BelowEarlier {
            percent: 31.03,
            then: Some("oMLX 0.7.3".into()),
            now: Some("oMLX 0.8.0".into()),
        }),
        ..CorrelationInsight::default()
    };
    sample
}

/// Where the golden files live, for reading and for `MLXTOP_BLESS=1`.
fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/tests/fixtures")
        .join(name)
}

/// `actual` must equal the golden file `name`. With `MLXTOP_BLESS=1` the
/// file is written instead, for a change that means to alter it.
fn golden_text(name: &str, actual: &str) {
    let path = fixture_path(name);
    if std::env::var_os("MLXTOP_BLESS").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    if actual != expected {
        let line = actual
            .lines()
            .zip(expected.lines())
            .position(|(a, e)| a != e)
            .unwrap_or_else(|| actual.lines().count().min(expected.lines().count()));
        panic!(
            "{name} differs at line {}:\n  actual:   {:?}\n  expected: {:?}",
            line + 1,
            actual.lines().nth(line),
            expected.lines().nth(line)
        );
    }
}

/// The static report for `sample`, read live a second ago.
fn static_report(mut sample: Sample) -> String {
    if sample.llm_observed_at.is_some() {
        sample.llm_observed_at = Some(SystemTime::now() - Duration::from_secs(1));
    }
    let mut out = Vec::new();
    crate::report::write_static(
        &mut out,
        &sample,
        1,
        crate::config::Thresholds::default(),
        Platform::MacOs,
    )
    .unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn the_static_report_reads_as_it_did_before_the_json_snapshot() {
    for (name, sample) in [
        ("once_slowdown.txt", mac_studio_slowdown()),
        ("once_stall.txt", mac_studio_stall()),
        ("once_idle.txt", mac_studio_idle()),
        ("once_no_runtime.txt", mac_no_runtime()),
        ("once_remote.txt", mac_remote_runtime()),
    ] {
        golden_text(name, &static_report(sample));
    }
}

// --- the JSON snapshot ------------------------------------------------------

use super::{snapshot, stream, write_line, SCHEMA_VERSION};
use serde_json::Value;
use std::collections::BTreeSet;
use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn json(sample: &Sample, platform: Platform) -> Value {
    serde_json::to_value(snapshot(sample, platform, 1)).unwrap()
}

fn line(sample: &Sample, platform: Platform) -> String {
    let mut out = Vec::new();
    write_line(&mut out, &snapshot(sample, platform, 1)).unwrap();
    String::from_utf8(out).unwrap()
}

/// Where `actual` and `expected` part, field by field: a value that
/// changed, a field the golden file has and the output lost, and a field
/// the output gained.
fn differences(path: &str, actual: &Value, expected: &Value, found: &mut Vec<String>) {
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => {
            for (key, value) in expected {
                let at = format!("{path}.{key}");
                match actual.get(key) {
                    Some(actual) => differences(&at, actual, value, found),
                    None => found.push(format!("{at}: missing from the output")),
                }
            }
            for key in actual.keys().filter(|key| !expected.contains_key(*key)) {
                found.push(format!(
                    "{path}.{key}: new; add it to the golden file and the user guide"
                ));
            }
        }
        (Value::Array(actual), Value::Array(expected)) if actual.len() == expected.len() => {
            for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                differences(&format!("{path}[{index}]"), actual, expected, found);
            }
        }
        (Value::Number(a), Value::Number(e))
            if (a.as_f64().unwrap() - e.as_f64().unwrap()).abs() < 1e-9 => {}
        _ if actual == expected => {}
        _ => found.push(format!("{path}: {actual} (golden: {expected})")),
    }
}

/// `snapshot` must equal the golden JSON file `name`, field by field. A
/// blessed file keeps the output's own field order.
fn golden_json(name: &str, snapshot: &super::Snapshot) {
    let path = fixture_path(name);
    if std::env::var_os("MLXTOP_BLESS").is_some() {
        let mut text = serde_json::to_string_pretty(snapshot).unwrap();
        text.push('\n');
        std::fs::write(&path, text).unwrap();
        return;
    }
    let actual = &serde_json::to_value(snapshot).unwrap();
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display())),
    )
    .unwrap();
    let mut found = Vec::new();
    differences("", actual, &expected, &mut found);
    assert!(found.is_empty(), "{name}:\n  {}", found.join("\n  "));
}

#[test]
fn the_snapshot_matches_the_golden_schema_field_by_field() {
    // A sample with a value in every field the others leave null: Splash's
    // own counts, and a session that knows the best decode reached here.
    golden_json(
        "export_v1.json",
        &super::session_snapshot(&watching(watched()), &splash_slowdown(), 1),
    );
}

#[test]
fn every_line_is_one_json_object_of_about_three_kilobytes() {
    let text = line(&mac_studio_slowdown(), Platform::MacOs);
    assert!(text.ends_with('\n'));
    assert_eq!(text.matches('\n').count(), 1, "one line: {text}");
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["schema_version"], SCHEMA_VERSION);
    assert_eq!(value["ts_ms"], TS);
    // About 2.5–4 KB: more while a slowdown and its requests are named, less
    // with no runtime and no model facts to report on.
    let busy = text.len();
    let idle = line(&mac_studio_idle(), Platform::MacOs).len();
    let quiet = line(&mac_no_runtime(), Platform::MacOs).len();
    assert!(
        (2_500..=3_500).contains(&idle) && busy <= 4_000 && quiet < idle && idle < busy,
        "busy {busy}, idle {idle}, quiet {quiet} bytes"
    );
    // The fullest line has its own bound, about 4.3 KB: on top of the busy
    // one, the runtime counts its own KV room and draft, so decode
    // efficiency, the context left and the session's best are known too,
    // and the session says what first tokens lost.
    let mut full = Vec::new();
    write_line(
        &mut full,
        &super::session_snapshot(&watching(watched()), &splash_slowdown(), 1),
    )
    .unwrap();
    assert!(
        busy < full.len() && full.len() <= 4_500,
        "busy {busy}, full {} bytes",
        full.len()
    );
}

#[test]
fn no_process_names_command_lines_or_ids_leave_the_machine() {
    let text = line(&mac_studio_slowdown(), Platform::MacOs);
    for private in [
        "omlx-worker",
        "sk-private-key",
        "/Users/someone",
        "Google Chrome",
        "4242",
    ] {
        assert!(!text.contains(private), "{private} leaked: {text}");
    }
    // Runtime and model names stay.
    assert!(text.contains("\"provider\":\"oMLX\""));
    assert!(text.contains("\"model\":\"Qwen3.8-27B-oQ4e-mtp\""));
    // With no runtime answering, the model is named after its process: that
    // name is not a model's, and stays on the machine.
    let mut sample = mac_no_runtime();
    sample.llm_count = 1;
    sample.llm_top = "llama-server".into();
    sample.llm_model = "llama-server".into();
    sample.llm_provider = "llama.cpp".into();
    let value = json(&sample, Platform::MacOs);
    assert_eq!(value["runtime"]["model"], Value::Null);
    assert_eq!(value["runtime"]["provider"], "llama.cpp");
    assert!(!value.to_string().contains("llama-server"));
}

#[test]
fn unknown_readings_are_null_never_zero() {
    // The first sample of a session: no rates yet, nothing read.
    let value = json(&Sample::default(), Platform::MacOs);
    for pointer in [
        "/ts_ms",
        "/host/ram_bytes",
        "/host/chip",
        "/host/memory/pressure",
        "/host/memory/tight",
        "/host/memory/available_ratio",
        "/host/memory/ram/used_bytes",
        "/host/memory/ram/left_bytes",
        "/host/memory/swap/total_bytes",
        "/host/memory/swap/in_bytes_per_s",
        "/host/memory/swap/growth_bytes_per_s",
        "/host/memory/file_paging/in_bytes_per_s",
        "/host/memory/compression/compress_bytes_per_s",
        "/host/memory/gpu/used_bytes",
        "/host/memory/gpu/limit_bytes",
        "/host/memory/gpu/left_bytes",
        "/host/gpus/0/busy_ratio",
        "/host/thermal/level",
        "/host/thermal/limited",
        "/host/thermal/hottest_celsius",
        "/host/system_power_w",
        "/runtime/provider",
        "/runtime/model",
        "/runtime/status",
        "/runtime/telemetry",
        "/runtime/observed_at_ms",
        "/runtime/connected",
        "/runtime/mlx",
        "/host/runtime_process/memory_bytes",
        "/host/runtime_process/cpu_cores",
        "/runtime/generation/tok_per_s",
        "/runtime/generation/basis",
        "/runtime/prefill/tok_per_s",
        "/runtime/requests/active",
        "/runtime/requests/prefilling",
        "/runtime/requests/context_tokens",
        "/runtime/cache/reuse_ratio",
        "/runtime/draft/acceptance_ratio",
        "/host/runtime_process/disk_read_bytes_per_s",
        "/host/published_bandwidth_bytes_per_s",
        "/host/published_bandwidth_source",
        "/runtime/context_allowed_tokens",
        "/runtime/kv_free_tokens",
        "/runtime/kv_free_bytes",
        "/runtime/preemptions_total",
        "/runtime/draft/tokens_per_pass",
        "/runtime/model_facts",
        "/efficiency",
        "/context_left",
        "/usual_speed",
        "/earlier_sessions",
        "/slowdown",
        "/slowdown_ended",
        "/lost_before_first_tokens",
    ] {
        assert_eq!(value.pointer(pointer), Some(&Value::Null), "{pointer}");
    }
    assert_eq!(value["verdict"]["state"], "collecting_baseline");
    assert_eq!(value["runtime"]["fresh"], false);
    assert_eq!(value["host"]["runtime_process"]["count"], 0);
    assert_eq!(value["runtime"]["requests"]["list"], serde_json::json!([]));
    assert_eq!(value["unusual"], serde_json::json!([]));
}

#[test]
fn rates_and_ratios_keep_their_units() {
    let value = json(&mac_studio_slowdown(), Platform::MacOs);
    assert_eq!(value["host"]["memory"]["swap"]["in_bytes_per_s"], 6 * MIB);
    assert_eq!(
        value["host"]["memory"]["swap"]["growth_bytes_per_s"],
        2 * MIB
    );
    assert_eq!(value["host"]["memory"]["available_ratio"], 0.24);
    assert_eq!(value["host"]["gpus"][0]["busy_ratio"], 0.96);
    assert_eq!(value["runtime"]["cache"]["reuse_ratio"], 0.915);
    assert_eq!(value["runtime"]["draft"]["acceptance_ratio"], 0.78);
    assert_eq!(value["host"]["system_power_w"], 35.712);
    assert_eq!(value["runtime"]["observed_at_ms"], TS - 250);
    assert_eq!(value["usual_speed"]["relative_change"], -0.4976);
    assert_eq!(value["host"]["runtime_process"]["cpu_cores"], 0.45);
    // A share that is not one is unknown, not clipped.
    let mut sample = mac_studio_slowdown();
    sample.llm_draft_acceptance = Some(1.4);
    sample.llm_generation_tps = Some(f64::NAN);
    let value = json(&sample, Platform::MacOs);
    assert_eq!(value["runtime"]["draft"]["acceptance_ratio"], Value::Null);
    assert_eq!(value["runtime"]["generation"]["tok_per_s"], Value::Null);
    assert_eq!(value["runtime"]["generation"]["basis"], Value::Null);
}

#[test]
fn the_verdict_is_the_one_the_screen_shows() {
    for sample in [
        mac_studio_slowdown(),
        mac_studio_stall(),
        mac_studio_idle(),
        mac_no_runtime(),
        mac_remote_runtime(),
    ] {
        let finding = crate::diagnosis::assess(&sample);
        let verdict = &json(&sample, Platform::MacOs)["verdict"];
        assert_eq!(verdict["title"], finding.title.as_str());
        assert_eq!(verdict["evidence"], finding.evidence.as_str());
        assert_eq!(verdict["context"], finding.context.as_str());
        assert_eq!(verdict["actionable"], finding.actionable);
        match finding.next.as_str() {
            "" => assert_eq!(verdict["next"], Value::Null),
            next => assert_eq!(verdict["next"], next),
        }
    }
    let state = |sample: &Sample| json(sample, Platform::MacOs)["verdict"]["state"].clone();
    let severity = |sample: &Sample| json(sample, Platform::MacOs)["verdict"]["severity"].clone();
    assert_eq!(state(&mac_studio_slowdown()), "generation_slowed");
    assert_eq!(severity(&mac_studio_slowdown()), "critical");
    assert_eq!(state(&mac_studio_stall()), "generation_stalled");
    assert_eq!(state(&mac_studio_idle()), "healthy");
    assert_eq!(severity(&mac_studio_idle()), "ok");
    assert_eq!(state(&mac_remote_runtime()), "requests_waiting");
    assert_eq!(severity(&mac_remote_runtime()), "warning");
    let mut prefill = mac_studio_slowdown();
    prefill.correlation.phase = Phase::Prefill;
    prefill.llm_prefill_tps_live = true;
    assert_eq!(state(&prefill), "prefill_slowed");
    let mut critical = mac_studio_idle();
    critical.pressure = "RED".into();
    assert_eq!(state(&critical), "memory_bottleneck");
    let mut unknown = mac_studio_idle();
    unknown.pressure = "UNKNOWN".into();
    unknown.pressure_meaning = "unavailable".into();
    assert_eq!(state(&unknown), "no_bottleneck");
    assert_eq!(severity(&unknown), "neutral");
}

#[test]
fn the_verdict_names_every_kind_of_headline() {
    let mut sample = mac_studio_idle();
    let mut state = |change: &dyn Fn(&mut Sample)| {
        change(&mut sample);
        crate::diagnosis::assess(&sample).verdict
    };
    use crate::diagnosis::Verdict;
    for (impact, verdict) in [
        ("MEMORY STRESS", Verdict::MemoryStress),
        ("SWAP THRASHING", Verdict::SwapThrashing),
        ("HEAVY PAGING", Verdict::HeavyPaging),
        ("PAGE-IN RECOVERY", Verdict::PageInRecovery),
        ("PAGING ACTIVE", Verdict::PagingActive),
        ("WATCH PAGING", Verdict::WatchPaging),
        ("COMPRESSION ACTIVE", Verdict::CompressionActive),
        ("THERMAL LIMIT", Verdict::ThermalLimit),
        ("DATA LIMITED", Verdict::CountersIncomplete),
    ] {
        assert_eq!(
            state(&|sample| sample.impact = impact.into()),
            verdict,
            "{impact}"
        );
    }
    let issue = |gpu_issue: crate::gpu_findings::GpuIssue| {
        move |sample: &mut Sample| {
            sample.impact = "LLM READY".into();
            sample.gpu_issue = Some(gpu_issue.clone());
        }
    };
    assert_eq!(
        state(&issue(crate::gpu_findings::GpuIssue::Offload {
            offloaded: GIB,
            size: 4 * GIB
        })),
        Verdict::CpuOffload
    );
    assert_eq!(
        state(&issue(crate::gpu_findings::GpuIssue::VramFull {
            index: 0,
            used: 23 * GIB,
            total: 24 * GIB,
            percent: 98
        })),
        Verdict::VramFull
    );
    assert_eq!(
        state(&issue(crate::gpu_findings::GpuIssue::Throttled {
            index: 1,
            cause: crate::gpu::Throttle::Thermal,
            temperature: Some(88)
        })),
        Verdict::GpuThrottled
    );
    let serialized = serde_json::to_value(Verdict::GpuThrottled).unwrap();
    assert_eq!(serialized, "gpu_throttled");
}

/// An NVIDIA workstation: a card with memory of its own, and a runtime
/// that keeps its readings in a log.
pub(crate) fn linux_workstation() -> Sample {
    Sample {
        total_memory: 64 * GIB,
        vm_available: true,
        wired: 6 * GIB,
        anonymous: 20 * GIB,
        file_backed: 30 * GIB,
        availability: Some(55),
        pressure: "GREEN".into(),
        pressure_meaning: "normal".into(),
        swap_available: true,
        swap_total: 8 * GIB,
        rate_ready: true,
        page_in: 9 * MIB,
        compress: 5 * MIB,
        gpu_util: Some(91),
        gpus: vec![
            crate::gpu::Device {
                index: 0,
                uuid: "GPU-aaaa".into(),
                vendor: crate::gpu::Vendor::Nvidia,
                name: "NVIDIA RTX 4090".into(),
                utilization: Some(91),
                used: Some(20 * GIB),
                total: Some(24 * GIB),
                temperature: Some(71),
                throttle_reasons: Some(0x20),
                unified: false,
                limits: Default::default(),
            },
            crate::gpu::Device {
                index: 1,
                uuid: "GPU-bbbb".into(),
                vendor: crate::gpu::Vendor::Amd,
                name: "AMD Radeon PRO W7900".into(),
                utilization: Some(12),
                used: Some(2 * GIB),
                total: Some(48 * GIB),
                temperature: None,
                throttle_reasons: None,
                unified: false,
                limits: Default::default(),
            },
        ],
        metal: MetalTelemetry {
            architecture: Some("x86_64".into()),
            ..MetalTelemetry::default()
        },
        thermal: "71°C measured".into(),
        llm_provider: "llama.cpp".into(),
        llm_model: "qwen3-32b-q4_k_m.gguf".into(),
        llm_status: "last result".into(),
        llm_source: TelemetrySource::Log,
        llm_generation_tps: Some(38.25),
        llm_prefill_tps: Some(1890.0),
        ts: Some(TS),
        ..Sample::default()
    }
}

#[test]
fn memory_follows_the_hardware() {
    // Apple Silicon: one pool. The GPU's part is RAM, held to its limit.
    let mac = json(&mac_studio_slowdown(), Platform::MacOs);
    assert_eq!(mac["host"]["memory"]["layout"], "unified");
    assert_eq!(mac["host"]["memory"]["vram"], Value::Null);
    assert_eq!(mac["host"]["memory"]["gpu"]["used_bytes"], 18 * GIB);
    assert_eq!(mac["host"]["memory"]["gpu"]["limit_bytes"], 28_000 * MIB);
    assert_eq!(mac["host"]["memory"]["gpu"]["limit_source"], "host");
    assert_eq!(
        mac["host"]["memory"]["gpu"]["left_bytes"],
        28_000 * MIB - 18 * GIB
    );
    assert_eq!(mac["host"]["memory"]["ram"]["used_bytes"], 27 * GIB);
    assert_eq!(mac["host"]["memory"]["ram"]["left_bytes"], 9 * GIB);
    assert_eq!(mac["host"]["gpus"].as_array().unwrap().len(), 1);
    assert_eq!(mac["host"]["gpus"][0]["vendor"], "apple");
    // Cards with memory of their own: VRAM beside RAM, each card listed.
    let linux = json(&linux_workstation(), Platform::Linux);
    assert_eq!(linux["host"]["memory"]["layout"], "dedicated");
    assert_eq!(linux["host"]["memory"]["gpu"], Value::Null);
    assert_eq!(linux["host"]["memory"]["vram"]["used_bytes"], 22 * GIB);
    assert_eq!(linux["host"]["memory"]["vram"]["total_bytes"], 72 * GIB);
    assert_eq!(linux["host"]["memory"]["vram"]["left_bytes"], 50 * GIB);
    assert_eq!(linux["host"]["gpus"][0]["vendor"], "nvidia");
    assert_eq!(linux["host"]["gpus"][0]["throttle"], "thermal");
    assert_eq!(linux["host"]["gpus"][0]["temperature_celsius"], 71);
    assert_eq!(linux["host"]["gpus"][1]["vendor"], "amd");
    // A driver that does not say why clocks are held is unknown.
    assert_eq!(linux["host"]["gpus"][1]["throttle"], Value::Null);
    assert_eq!(linux["host"]["chip"], Value::Null);
    assert_eq!(linux["host"]["thermal"]["hottest_celsius"], 71);
    // Linux has no compressor and reads no file paging: unknown, not zero.
    assert_eq!(
        linux["host"]["memory"]["ram"]["compressed_bytes"],
        Value::Null
    );
    assert_eq!(linux["host"]["memory"]["ram"]["wired_bytes"], 6 * GIB);
    assert_eq!(
        linux["host"]["memory"]["file_paging"]["in_bytes_per_s"],
        Value::Null
    );
    assert_eq!(
        linux["host"]["memory"]["compression"]["compress_bytes_per_s"],
        Value::Null
    );
    // An integrated GPU without VRAM (DGX Spark) shares RAM: one pool.
    let mut spark = linux_workstation();
    spark.gpus.truncate(1);
    spark.gpus[0].used = None;
    spark.gpus[0].total = None;
    spark.gpus[0].unified = true;
    let spark = json(&spark, Platform::Linux);
    assert_eq!(spark["host"]["memory"]["layout"], "unified");
    assert_eq!(spark["host"]["memory"]["gpu"]["used_bytes"], Value::Null);
    // No GPU at all: RAM alone, and no layout to name.
    let mut bare = linux_workstation();
    bare.gpus.clear();
    let bare = json(&bare, Platform::Linux);
    assert_eq!(bare["host"]["memory"]["layout"], Value::Null);
    assert_eq!(bare["host"]["gpus"], serde_json::json!([]));
    // Windows reports only what is in use and what is available.
    let windows = Sample {
        total_memory: 32 * GIB,
        vm_available: true,
        anonymous: 24 * GIB,
        availability: Some(25),
        paging_unavailable: true,
        swap_available: true,
        rate_ready: true,
        ..Sample::default()
    };
    let windows = json(&windows, Platform::Windows);
    assert_eq!(windows["host"]["memory"]["ram"]["used_bytes"], 24 * GIB);
    assert_eq!(windows["host"]["memory"]["ram"]["left_bytes"], 8 * GIB);
    for kind in ["wired_bytes", "app_bytes", "file_cache_bytes", "free_bytes"] {
        assert_eq!(
            windows["host"]["memory"]["ram"][kind],
            Value::Null,
            "{kind}"
        );
    }
    assert_eq!(
        windows["host"]["memory"]["swap"]["in_bytes_per_s"],
        Value::Null
    );
    assert_eq!(windows["host"]["memory"]["swap"]["growth_bytes_per_s"], 0);
}

#[test]
fn a_cards_clock_limit_is_named_or_unknown() {
    let mut sample = linux_workstation();
    for (reasons, name) in [
        (Some(0x20), "thermal"),
        (Some(0x80), "power_brake"),
        (Some(0x8), "hardware_slowdown"),
        (Some(0x4), "power_cap"),
        (Some(0x1), "none"),
    ] {
        sample.gpus[0].throttle_reasons = reasons;
        assert_eq!(
            json(&sample, Platform::Linux)["host"]["gpus"][0]["throttle"],
            name
        );
    }
}

#[test]
fn the_gpu_limit_says_where_it_came_from() {
    let mut sample = mac_studio_slowdown();
    sample.metal.resource_limit = None;
    sample.mlx.recommended_working_set = Some(26 * GIB);
    let value = json(&sample, Platform::MacOs);
    assert_eq!(value["host"]["memory"]["gpu"]["limit_bytes"], 26 * GIB);
    assert_eq!(value["host"]["memory"]["gpu"]["limit_source"], "runtime");
    // Over the limit there is no room left, not a negative amount.
    sample.gpu_in_use = Some(27 * GIB);
    assert_eq!(
        json(&sample, Platform::MacOs)["host"]["memory"]["gpu"]["left_bytes"],
        0
    );
    // The runtime's own Metal limit, taken where macOS sets none.
    sample.mlx.resource_limit = Some(25 * GIB);
    sample.metal.resource_limit = Some(25 * GIB);
    let value = json(&sample, Platform::MacOs);
    assert_eq!(value["host"]["memory"]["gpu"]["limit_source"], "runtime");
}

#[test]
fn each_rate_says_what_kind_of_rate_it_is() {
    let value = json(&mac_studio_slowdown(), Platform::MacOs);
    assert_eq!(value["runtime"]["generation"]["basis"], "live");
    assert_eq!(value["runtime"]["prefill"]["basis"], "average");
    let value = json(&linux_workstation(), Platform::Linux);
    assert_eq!(value["runtime"]["telemetry"], "completion_log");
    assert_eq!(value["runtime"]["generation"]["tok_per_s"], 38.25);
    assert_eq!(value["runtime"]["generation"]["basis"], "last_request");
    // mlx-serve keeps the last finished request's prefill speed.
    let mut sample = mac_studio_idle();
    sample.llm_provider = "mlx-serve".into();
    assert_eq!(
        json(&sample, Platform::MacOs)["runtime"]["prefill"]["basis"],
        "last_request"
    );
    sample.llm_source = TelemetrySource::Report;
    sample.llm_provider = "Ollama".into();
    assert_eq!(
        json(&sample, Platform::MacOs)["runtime"]["generation"]["basis"],
        "last_request"
    );
    assert_eq!(
        json(&sample, Platform::MacOs)["runtime"]["telemetry"],
        "reported_usage"
    );
    sample.llm_source = TelemetrySource::None;
    assert_eq!(
        json(&sample, Platform::MacOs)["runtime"]["generation"]["basis"],
        Value::Null
    );
}

#[test]
fn requests_say_what_each_is_doing() {
    let mut sample = mac_studio_slowdown();
    let request = |id: &str, queued: bool, prefilled: Option<u64>, completed: bool| RequestUsage {
        id: id.into(),
        prompt: 1_000,
        queued,
        prefilled,
        completed,
        ..RequestUsage::default()
    };
    sample.llm_requests.push(request("b", true, None, false));
    sample
        .llm_requests
        .push(request("c", false, Some(400), false));
    sample.llm_requests.push(request("d", false, None, true));
    sample.llm_requests[0].idle_for = Some(Duration::from_millis(1_250));
    let value = json(&sample, Platform::MacOs);
    let states: Vec<&str> = value["runtime"]["requests"]["list"]
        .as_array()
        .unwrap()
        .iter()
        .map(|request| request["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["generating", "queued", "prefilling", "completed"]);
    assert_eq!(value["runtime"]["requests"]["prefilling"], 1);
    assert_eq!(value["runtime"]["requests"]["generating"], 1);
    assert_eq!(value["runtime"]["requests"]["context_tokens"], 41_023);
    assert_eq!(value["runtime"]["requests"]["list"][0]["idle_s"], 1.25);
    assert_eq!(
        value["runtime"]["requests"]["list"][2]["prefilled_tokens"],
        400
    );
}

#[test]
fn a_slowdown_names_its_cause_until_it_ends() {
    let value = json(&mac_studio_slowdown(), Platform::MacOs);
    let slowdown = &value["slowdown"];
    assert_eq!(slowdown["cause"], "paging");
    assert_eq!(slowdown["stalled"], false);
    assert_eq!(slowdown["usual_tok_per_s"], 123.0);
    assert_eq!(slowdown["lowest_tok_per_s"], 58.4);
    assert_eq!(slowdown["duration_s"], 12.0);
    assert_eq!(value["unusual"][0]["cause"], "paging");
    let stall = json(&mac_studio_stall(), Platform::MacOs);
    assert_eq!(stall["slowdown"]["stalled"], true);
    assert_eq!(stall["slowdown"]["cause"], "stall");
    assert_eq!(stall["slowdown"]["stalled_s"], 4.0);
    assert_eq!(stall["usual_speed"]["relative_change"], Value::Null);
    // The sample where it ends says how; the next says nothing.
    for (settled, finished, how) in [
        (false, false, "recovered"),
        (true, false, "settled"),
        (false, true, "finished"),
    ] {
        let mut sample = mac_studio_idle();
        sample.correlation.ended = Some(crate::domain::DropEnd {
            settled,
            finished,
            seconds: 21.5,
            low: 58.4,
            rate: 119.0,
            reference: 123.0,
        });
        let value = json(&sample, Platform::MacOs);
        assert_eq!(value["slowdown"], Value::Null);
        assert_eq!(value["slowdown_ended"]["how"], how);
        assert_eq!(value["slowdown_ended"]["duration_s"], 21.5);
        assert_eq!(value["slowdown_ended"]["tok_per_s"], 119.0);
    }
    // Every reason has a name of its own.
    let names: BTreeSet<&str> = CAUSES.into_iter().map(super::cause_name).collect();
    assert_eq!(names.len(), CAUSES.len());
}

const CAUSES: [CorrelationCause; 16] = [
    CorrelationCause::None,
    CorrelationCause::Stall,
    CorrelationCause::Paging,
    CorrelationCause::Compression,
    CorrelationCause::MemoryPressure,
    CorrelationCause::Thermal,
    CorrelationCause::MetalMemory,
    CorrelationCause::Queueing,
    CorrelationCause::ConcurrentPrefill,
    CorrelationCause::ModelMemory,
    CorrelationCause::DiskIo,
    CorrelationCause::Cpu,
    CorrelationCause::GpuIdle,
    CorrelationCause::DraftAcceptance,
    CorrelationCause::KvPreemption,
    CorrelationCause::EarlierSessions,
];

const VERDICTS: [crate::diagnosis::Verdict; 20] = {
    use crate::diagnosis::Verdict::*;
    [
        Healthy,
        NoBottleneck,
        CollectingBaseline,
        CountersIncomplete,
        GenerationSlowed,
        PrefillSlowed,
        GenerationStalled,
        RequestsWaiting,
        MemoryBottleneck,
        MemoryStress,
        SwapThrashing,
        HeavyPaging,
        PageInRecovery,
        PagingActive,
        WatchPaging,
        CompressionActive,
        ThermalLimit,
        CpuOffload,
        VramFull,
        GpuThrottled,
    ]
};

#[test]
fn heat_and_the_runtimes_allocator_are_read_when_reported() {
    let mut sample = mac_studio_idle();
    sample.thermal_state = ThermalState {
        level: Some(ThermalLevel::Heavy),
        cpu_speed_limit: Some(70),
    };
    sample.mlx.version = Some("0.30.1".into());
    sample.mlx.active_memory = Some(17 * GIB);
    sample.runtime.provider = Some("oMLX".into());
    sample.runtime.begin();
    sample.runtime.finish(true);
    let value = json(&sample, Platform::MacOs);
    assert_eq!(value["host"]["thermal"]["level"], "heavy");
    assert_eq!(value["host"]["thermal"]["limited"], true);
    assert_eq!(value["host"]["thermal"]["cpu_speed_limit_ratio"], 0.7);
    assert_eq!(value["runtime"]["mlx"]["version"], "0.30.1");
    assert_eq!(value["runtime"]["mlx"]["active_bytes"], 17 * GIB);
    assert_eq!(value["runtime"]["mlx"]["cache_bytes"], Value::Null);
    assert_eq!(value["runtime"]["connected"], true);
    // A runtime that does not answer is down; discovery that found none is
    // not a runtime at all.
    sample.runtime.finish(false);
    assert_eq!(
        json(&sample, Platform::MacOs)["runtime"]["connected"],
        false
    );
    sample.runtime.provider = None;
    assert_eq!(
        json(&sample, Platform::MacOs)["runtime"]["connected"],
        Value::Null
    );
}

#[test]
fn the_chips_published_bandwidth_is_an_estimate() {
    // The owner's 32-core M4 Max reads 410 GB/s, not the 40-core part's 546.
    let mac = json(&mac_studio_slowdown(), Platform::MacOs);
    assert_eq!(
        mac["host"]["published_bandwidth_bytes_per_s"],
        410_000_000_000_u64
    );
    assert_eq!(mac["host"]["published_bandwidth_source"], "estimate");
    // A bin the readings cannot settle is unknown, not guessed.
    let mut unsure = mac_studio_slowdown();
    unsure.metal.gpu_cores = None;
    let unsure = json(&unsure, Platform::MacOs);
    for field in [
        "published_bandwidth_bytes_per_s",
        "published_bandwidth_source",
    ] {
        assert_eq!(unsure["host"][field], Value::Null, "{field}");
    }
    // One card has its own figure; several cards have no single one.
    let mut one = linux_workstation();
    one.gpus.truncate(1);
    assert_eq!(
        json(&one, Platform::Linux)["host"]["published_bandwidth_bytes_per_s"],
        1_008_000_000_000_u64
    );
    assert_eq!(
        json(&linux_workstation(), Platform::Linux)["host"]["published_bandwidth_bytes_per_s"],
        Value::Null
    );
}

#[test]
fn model_facts_are_an_estimate_that_names_the_runtimes_own_figures() {
    let value = json(&mac_studio_slowdown(), Platform::MacOs);
    let facts = &value["runtime"]["model_facts"];
    assert_eq!(facts["source"], "estimate");
    assert_eq!(facts["weight_bytes_per_pass"], 15_020_479_488_u64);
    assert_eq!(facts["bits_per_weight"], 4.691);
    assert_eq!(facts["kv_bytes_per_token"], 33_280);
    assert_eq!(facts["kv_bytes_per_token_source"], "estimate");
    assert_eq!(facts["state_bytes_per_sequence"], 160_038_912);
    assert_eq!(facts["context_trained_tokens"], 262_144);
    assert_eq!(facts["draft_weight_bytes"], 3_848_808_960_u64);
    // The context the runtime allows is its own setting, beside its other limits.
    assert_eq!(value["runtime"]["context_allowed_tokens"], 131_072);
    // KV bytes the runtime reports win over the layers, and say whose they are.
    let mut sample = mac_studio_slowdown();
    sample.model.kv_bytes_per_token_reported = Some(65_536);
    let facts = &json(&sample, Platform::MacOs)["runtime"]["model_facts"];
    assert_eq!(facts["kv_bytes_per_token"], 65_536);
    assert_eq!(facts["kv_bytes_per_token_source"], "runtime");
    // A model with no fixed state holds 0 a sequence: known, not unknown.
    sample.model = crate::model_facts::ModelFacts {
        fixed_state_bytes: Some(0),
        ..Default::default()
    };
    let facts = &json(&sample, Platform::MacOs)["runtime"]["model_facts"];
    assert_eq!(facts["state_bytes_per_sequence"], 0);
    assert_eq!(facts["kv_bytes_per_token"], Value::Null);
    assert_eq!(facts["kv_bytes_per_token_source"], Value::Null);
    // With none of its figures known, there is no object at all.
    sample.model = crate::model_facts::ModelFacts {
        context_configured: Some(8_192),
        bits_per_weight: Some(f64::NAN),
        runtime_version: Some("oMLX 0.7.3".into()),
        ..Default::default()
    };
    let value = json(&sample, Platform::MacOs);
    assert_eq!(value["runtime"]["model_facts"], Value::Null);
    assert_eq!(value["runtime"]["context_allowed_tokens"], 8_192);
}

#[test]
fn decode_and_context_left_are_estimates_beside_what_the_runtime_counts() {
    let sample = splash_slowdown();
    let value = json(&sample, Platform::MacOs);
    // What Splash counts of its own work is the runtime's.
    let runtime = &value["runtime"];
    assert_eq!(runtime["source"], "runtime");
    assert_eq!(runtime["draft"]["tokens_per_pass"], 7.8);
    assert_eq!(runtime["kv_free_tokens"], 110_528);
    assert_eq!(runtime["kv_free_bytes"], 110_528_u64 * 33_280);
    assert_eq!(runtime["preemptions_total"], 2);
    // Decode per verify pass is mlxtop's estimate, as Diagnostics gives it:
    // 61.8 tok/s at 7.8 tokens a pass is 7.92 passes a second, each reading
    // the weights, 41,023 tokens of KV cache and the state: 32% of 410 GB/s.
    let decode = &value["efficiency"];
    assert_eq!(decode["source"], "estimate");
    assert_eq!(decode["draft"], true);
    assert_eq!(decode["verify_passes_per_s"], 7.92);
    assert_eq!(decode["tokens_per_pass"], 7.8);
    assert_eq!(
        decode["bytes_per_pass"],
        15_020_479_488_u64 + 41_023 * 33_280 + 160_038_912
    );
    assert_eq!(decode["bandwidth_share_ratio"], 0.3197);
    assert_eq!(decode["no_draft_ceiling_tok_per_s"], 24.78);
    let estimate = crate::efficiency::of_sample(&sample).unwrap();
    assert!(crate::efficiency::share_line(&estimate, None)
        .starts_with("Decode estimate: 32% of 410 GB/s per verify pass"));
    // `--once` watches no session, so it knows no best.
    assert_eq!(decode["best_bandwidth_share_ratio"], Value::Null);
    // Without a draft a pass emits one token; a share above the peak means
    // an input is wrong, and is unknown.
    let mut plain = sample.clone();
    plain.model.draft = Some(false);
    plain.llm_tokens_per_pass = None;
    plain.llm_draft_acceptance = None;
    plain.llm_generation_tps = Some(20.0);
    let decode = &json(&plain, Platform::MacOs)["efficiency"];
    assert_eq!(decode["draft"], false);
    assert_eq!(decode["tokens_per_pass"], 1.0);
    assert_eq!(decode["verify_passes_per_s"], 20.0);
    assert_eq!(decode["bandwidth_share_ratio"], 0.8071);
    plain.llm_generation_tps = Some(61.8);
    assert_eq!(json(&plain, Platform::MacOs)["efficiency"], Value::Null);
    // The context left is a forecast: Splash's 110.5k free tokens are more
    // than the 131.1k window less the 41.0k the request holds.
    let left = &value["context_left"];
    assert_eq!(left["source"], "estimate");
    assert_eq!(left["requests"], 1);
    assert_eq!(left["by_memory_tokens"], 110_528);
    assert_eq!(left["by_window_tokens"], 131_072 - 41_023);
    assert_eq!(left["tokens_per_request"], 131_072 - 41_023);
    assert_eq!(left["limited_by"], "window");
}

#[test]
fn the_context_left_names_the_limit_that_gives_it() {
    let left = |sample: &Sample| json(sample, Platform::MacOs)["context_left"].clone();
    // Less room than the window leaves: memory is the limit, and where the
    // two tie it is memory, as Diagnostics calls it the smaller.
    let mut sample = splash_slowdown();
    for (free, limited_by) in [(20_000, "memory"), (90_049, "memory"), (90_050, "window")] {
        sample.llm_kv_free_tokens = Some(free);
        let left = left(&sample);
        assert_eq!(left["tokens_per_request"], free.min(90_049), "{free}");
        assert_eq!(left["limited_by"], limited_by, "{free}");
    }
    // Two requests in flight share the room, and each has the window less
    // the longer of them.
    sample.llm_kv_free_tokens = Some(110_528);
    sample.llm_requests.push(RequestUsage {
        id: "req-8b2c".into(),
        prompt: 60_000,
        output: Some(1_000),
        ..RequestUsage::default()
    });
    let two = left(&sample);
    assert_eq!(two["requests"], 2);
    assert_eq!(two["by_memory_tokens"], 110_528 / 2);
    assert_eq!(two["by_window_tokens"], 131_072 - 61_000);
    assert_eq!(two["limited_by"], "memory");
    // llama.cpp sized its KV cache for the window when it loaded the model:
    // only the window limits it, and memory gives no figure.
    let mut sized = splash_slowdown();
    sized.llm_provider = "llama.cpp".into();
    let sized = left(&sized);
    assert_eq!(sized["by_memory_tokens"], Value::Null);
    assert_eq!(sized["tokens_per_request"], 131_072 - 41_023);
    assert_eq!(sized["limited_by"], "window");
}

#[test]
fn only_a_session_that_watched_similar_work_knows_the_best_decode() {
    let best = |decode: crate::efficiency::Record| {
        let snapshot = super::session_snapshot(&watching(decode), &splash_slowdown(), 1);
        serde_json::to_value(snapshot).unwrap()["efficiency"]["best_bandwidth_share_ratio"].clone()
    };
    let seen = |sample: &Sample, times: usize| {
        let mut record = crate::efficiency::Record::default();
        for _ in 0..times {
            record.observe(sample);
        }
        record
    };
    // Twenty samples at the usual 123 tok/s read 64% of 410 GB/s, against
    // the 32% the slowdown reads.
    assert_eq!(best(watched()), 0.6364);
    // Nineteen are too few; work at a quarter of the depth is not similar;
    // and another model's decode says nothing of this one's.
    assert_eq!(best(seen(&splash_decoding(), 19)), Value::Null);
    let mut shallow = splash_decoding();
    shallow.llm_requests[0].prompt = 10_000;
    assert_eq!(best(seen(&shallow, 20)), Value::Null);
    let mut other = splash_decoding();
    other.llm_model = "Qwen3.8-27B-oQ8".into();
    assert_eq!(best(seen(&other, 20)), Value::Null);
}

#[test]
fn with_a_session_the_verdict_is_the_one_the_dashboard_shows() {
    // Decoding near the best this session has seen, the dashboard notes
    // what would make it faster; `--once` has no session to know that.
    let sample = splash_decoding();
    let streamed =
        serde_json::to_value(super::session_snapshot(&watching(watched()), &sample, 1)).unwrap();
    let finding = crate::diagnosis::assess_session(&sample, &watched());
    assert_eq!(streamed["verdict"]["state"], "healthy");
    assert_eq!(streamed["verdict"]["next"], finding.next.as_str());
    assert_eq!(
        streamed["verdict"]["next"],
        "Decode is near its estimated limit: fewer bytes/token or a better draft."
    );
    assert_eq!(streamed["verdict"]["actionable"], false);
    assert_eq!(
        streamed["efficiency"]["best_bandwidth_share_ratio"],
        streamed["efficiency"]["bandwidth_share_ratio"]
    );
    let once = json(&sample, Platform::MacOs);
    assert_eq!(once["verdict"]["state"], "healthy");
    assert_eq!(once["verdict"]["next"], Value::Null);
}

#[test]
fn without_the_runtimes_own_counts_the_estimates_are_null() {
    // oMLX keeps Splash's status private: with a draft but no tokens per
    // pass there is no decode estimate, and with no KV room no context
    // left, session or not.
    let snapshot = super::session_snapshot(&watching(watched()), &mac_studio_slowdown(), 1);
    let value = serde_json::to_value(snapshot).unwrap();
    for pointer in [
        "/efficiency",
        "/context_left",
        "/runtime/kv_free_tokens",
        "/runtime/kv_free_bytes",
        "/runtime/preemptions_total",
        "/runtime/draft/tokens_per_pass",
    ] {
        assert_eq!(value.pointer(pointer), Some(&Value::Null), "{pointer}");
    }
    // A pass emits at least the token the model adds: fewer is no count.
    let mut odd = splash_slowdown();
    odd.llm_tokens_per_pass = Some(0.5);
    let value = json(&odd, Platform::MacOs);
    assert_eq!(value["runtime"]["draft"]["tokens_per_pass"], Value::Null);
    assert_eq!(value["efficiency"], Value::Null);
    // None preempted is a count, not an unknown.
    let mut none = splash_slowdown();
    none.llm_preemptions_total = Some(0);
    assert_eq!(
        json(&none, Platform::MacOs)["runtime"]["preemptions_total"],
        0
    );
}

#[test]
fn only_a_session_with_a_runtime_says_what_first_tokens_lost() {
    // A session whose requests lost time every way the summary counts it.
    let streamed = serde_json::to_value(super::session_snapshot(
        &watching(watched()),
        &splash_slowdown(),
        1,
    ))
    .unwrap();
    assert_eq!(
        streamed["lost_before_first_tokens"],
        serde_json::json!({
            "source": "estimate",
            "cache_misses_s": 124.0,
            "model_loads_s": 6.5,
            "overlap_s": 40.0,
            "overlap_told": true
        })
    );
    // A session that lost nothing says so: none of it is unknown. An
    // overlap not told costs nothing yet.
    let mut quiet = watching(watched());
    quiet.session.request_findings = Default::default();
    let lost = &serde_json::to_value(super::session_snapshot(&quiet, &splash_slowdown(), 1))
        .unwrap()["lost_before_first_tokens"];
    for field in ["cache_misses_s", "model_loads_s", "overlap_s"] {
        assert_eq!(lost[field], 0.0, "{field}");
    }
    assert_eq!(lost["overlap_told"], false);
    // `--once` watches no session; with no runtime found, nothing is said
    // about one.
    assert_eq!(
        json(&splash_slowdown(), Platform::MacOs)["lost_before_first_tokens"],
        Value::Null
    );
    let none = serde_json::to_value(super::session_snapshot(
        &watching(watched()),
        &mac_no_runtime(),
        1,
    ))
    .unwrap();
    assert_eq!(none["lost_before_first_tokens"], Value::Null);
}

#[test]
fn the_finding_below_earlier_sessions_gives_its_share_and_the_update() {
    let value = json(&below_earlier_sessions(), Platform::MacOs);
    assert_eq!(
        value["earlier_sessions"],
        serde_json::json!({
            "source": "estimate",
            "below_ratio": 0.3103,
            "runtime_from": "oMLX 0.7.3",
            "runtime_to": "oMLX 0.8.0"
        })
    );
    assert_eq!(value["slowdown"]["cause"], "earlier_sessions");
    // The same version slowed, or a version not known then or now: there
    // is no update to name.
    for (then, now) in [
        (Some("oMLX 0.8.0"), Some("oMLX 0.8.0")),
        (None, Some("oMLX 0.8.0")),
        (Some("oMLX 0.7.3"), None),
    ] {
        let mut sample = below_earlier_sessions();
        let below = sample.correlation.earlier.as_mut().unwrap();
        below.then = then.map(str::to_owned);
        below.now = now.map(str::to_owned);
        let earlier = &json(&sample, Platform::MacOs)["earlier_sessions"];
        assert_eq!(earlier["below_ratio"], 0.3103);
        assert_eq!(earlier["runtime_from"], Value::Null, "{then:?} {now:?}");
        assert_eq!(earlier["runtime_to"], Value::Null, "{then:?} {now:?}");
    }
    // The first judged sample below them says so before it is a slowdown.
    let mut first = below_earlier_sessions();
    let below = first.correlation.earlier.clone();
    first.correlation = CorrelationInsight {
        direction: ThroughputDirection::Flat,
        slow: true,
        delta_percent: Some(-31.03),
        unusual: vec![CorrelationCause::EarlierSessions],
        notes: vec![below.as_ref().unwrap().sentence()],
        usual_speed: Some((58.0, 46.4)),
        usual_for: "1 request · 41.0k context".into(),
        earlier: below,
        ..CorrelationInsight::default()
    };
    let value = json(&first, Platform::MacOs);
    assert_eq!(value["earlier_sessions"]["below_ratio"], 0.3103);
    assert_eq!(value["slowdown"], Value::Null);
    assert_eq!(value["unusual"][0]["cause"], "earlier_sessions");
    // Judged against this session's own usual, or slowed for a reason of
    // its own: no finding.
    assert_eq!(
        json(&mac_studio_slowdown(), Platform::MacOs)["earlier_sessions"],
        Value::Null
    );
}

/// Every field path in `value`: `memory.ram.left_bytes`, `gpus[].name`.
fn field_paths(prefix: &str, value: &Value, paths: &mut BTreeSet<String>) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                if !value.is_object() {
                    paths.insert(path.clone());
                }
                field_paths(&path, value, paths);
            }
        }
        Value::Array(items) => {
            for item in items {
                field_paths(&format!("{prefix}[]"), item, paths);
            }
        }
        _ => {}
    }
}

#[test]
fn the_user_guide_lists_every_field() {
    let mut ended = mac_studio_idle();
    ended.correlation.ended = Some(crate::domain::DropEnd {
        settled: false,
        finished: false,
        seconds: 1.0,
        low: 1.0,
        rate: 2.0,
        reference: 2.0,
    });
    ended.mlx.version = Some("0.30.1".into());
    let mut paths = BTreeSet::new();
    for (sample, platform) in [
        (mac_studio_slowdown(), Platform::MacOs),
        (splash_slowdown(), Platform::MacOs),
        (ended, Platform::MacOs),
        (linux_workstation(), Platform::Linux),
        (below_earlier_sessions(), Platform::MacOs),
    ] {
        field_paths("", &json(&sample, platform), &mut paths);
    }
    // What only a session knows.
    let streamed = super::session_snapshot(&watching(watched()), &splash_slowdown(), 1);
    field_paths("", &serde_json::to_value(streamed).unwrap(), &mut paths);
    let guide = include_str!("../../docs/USER_GUIDE.md");
    // And every name a verdict or a reason can take.
    for verdict in VERDICTS {
        paths.insert(
            serde_json::to_value(verdict)
                .unwrap()
                .as_str()
                .unwrap()
                .into(),
        );
    }
    paths.extend(CAUSES.map(|cause| super::cause_name(cause).to_owned()));
    let missing: Vec<&String> = paths
        .iter()
        .filter(|path| !guide.contains(&format!("`{path}`")))
        .collect();
    assert!(missing.is_empty(), "not in the user guide: {missing:?}");
}

// --- the stream -------------------------------------------------------------

/// A pipe whose reader takes `lines` lines and goes away, as `head` does.
struct Pipe {
    taken: Vec<u8>,
    lines: usize,
    kind: io::ErrorKind,
    /// Lines taken so far, for a signal that ends the stream.
    count: Arc<AtomicUsize>,
}

impl Pipe {
    fn new(lines: usize) -> Self {
        Self {
            taken: Vec::new(),
            lines,
            kind: io::ErrorKind::BrokenPipe,
            count: Arc::default(),
        }
    }

    fn lines(&self) -> Vec<Value> {
        String::from_utf8(self.taken.clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

impl Write for Pipe {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.count.load(Ordering::SeqCst) >= self.lines {
            return Err(io::Error::from(self.kind));
        }
        self.taken.extend_from_slice(bytes);
        self.count.store(
            self.taken.iter().filter(|byte| **byte == b'\n').count(),
            Ordering::SeqCst,
        );
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn collector() -> crate::collector::Collector {
    crate::collector::Collector::with_host(
        60,
        crate::test_support::offline_config(),
        Box::new(crate::test_support::linux_host()),
        Platform::Linux,
        None,
    )
}

#[test]
fn the_stream_writes_one_json_line_per_sample_and_ends_quietly_when_the_reader_goes() {
    let mut collector = collector();
    let mut pipe = Pipe::new(3);
    let ended = stream(&mut collector, Duration::ZERO, &mut pipe, &|| false);
    assert!(ended.is_ok(), "a closed pipe is the end, not an error");
    let lines = pipe.lines();
    assert_eq!(lines.len(), 3);
    assert!(pipe.taken.ends_with(b"\n"), "no line is cut short");
    for line in &lines {
        assert_eq!(line["schema_version"], SCHEMA_VERSION);
        assert!(line["ts_ms"].is_u64());
        assert_eq!(line["host"]["platform"], "linux");
        // The first sample only starts the rates: every line has them.
        assert!(
            line["host"]["memory"]["swap"]["in_bytes_per_s"].is_u64(),
            "{line}"
        );
    }
    // A sample to start the rates, three written, one the reader refused.
    assert_eq!(collector.current.sequence, 5);
}

#[test]
fn a_signal_ends_the_stream_between_lines() {
    let mut collector = collector();
    let mut pipe = Pipe::new(usize::MAX);
    let count = pipe.count.clone();
    let stop = move || count.load(Ordering::SeqCst) >= 2;
    stream(&mut collector, Duration::from_millis(30), &mut pipe, &stop).unwrap();
    assert_eq!(pipe.lines().len(), 2);
    // Asked to stop before it starts, it writes nothing.
    let mut pipe = Pipe::new(usize::MAX);
    stream(&mut collector, Duration::from_secs(60), &mut pipe, &|| true).unwrap();
    assert!(pipe.taken.is_empty());
}

#[test]
fn a_failed_write_other_than_a_closed_pipe_is_an_error() {
    let mut pipe = Pipe::new(1);
    pipe.kind = io::ErrorKind::PermissionDenied;
    let error = stream(&mut collector(), Duration::ZERO, &mut pipe, &|| false).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(pipe.lines().len(), 1);
}

#[test]
fn once_with_json_prints_the_snapshot_and_nothing_else() {
    let mut out = Vec::new();
    crate::cli::run_once(&mut collector(), Duration::ZERO, true, &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert_eq!(text.matches('\n').count(), 1, "{text}");
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["schema_version"], SCHEMA_VERSION);
    assert_eq!(value["interval_s"], 0);
    assert!(value["host"]["memory"]["swap"]["in_bytes_per_s"].is_u64());
    assert_eq!(value["host"]["ram_bytes"], 16_000_000_u64 * 1024);
}
