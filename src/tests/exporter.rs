// SPDX-License-Identifier: MIT
// `mlxtop serve`: the Prometheus text against a golden file and the user
// guide, unknown readings left out, what is written while sampling is not
// current, the request rules (a loopback Host, GET and HEAD only, a head of
// 8 KiB within 2 s) over real loopback sockets, `--listen` refusing every
// address but loopback, and serving that never samples for a request and
// ends cleanly.
use super::*;
use crate::cli::{help_text, parse_args, CliAction};
use crate::config::Config;
use crate::domain::Sample;
use crate::export::tests::{
    linux_workstation, mac_no_runtime, mac_studio_slowdown, mac_studio_stall, splash_slowdown,
    watched, watching,
};
use crate::host::Platform;
use crate::test_support::{linux_host, offline_config, FakeHost};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

fn taken(sample: &Sample, platform: Platform) -> Taken {
    Taken {
        snapshot: crate::export::snapshot(sample, platform, 1),
        at: Instant::now(),
    }
}

/// The text for `sample`, a quarter of a second after it was taken.
fn text(sample: &Sample, platform: Platform) -> String {
    let taken = taken(sample, platform);
    metrics(Some(&taken), taken.at + Duration::from_millis(250))
}

/// The text `serve` answers for `sample` on a Mac whose session has watched
/// decode reach its best, a quarter of a second after the sample.
fn served(sample: &Sample) -> String {
    let taken = Taken {
        snapshot: crate::export::session_snapshot(&watching(watched()), sample, 1),
        at: Instant::now(),
    };
    metrics(Some(&taken), taken.at + Duration::from_millis(250))
}

/// Each sample line's value, by its name and labels.
fn values(text: &str) -> BTreeMap<String, f64> {
    text.lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let (series, value) = line.rsplit_once(' ').unwrap();
            (series.to_owned(), value.parse().unwrap())
        })
        .collect()
}

/// The families written, by the names their TYPE lines give.
fn families(text: &str) -> Vec<&str> {
    text.lines()
        .filter_map(|line| line.strip_prefix("# TYPE "))
        .map(|rest| rest.split(' ').next().unwrap())
        .collect()
}

#[test]
fn the_metrics_match_the_golden_text() {
    // Every reading known: Splash's own counts, and a session's best.
    let actual = served(&splash_slowdown());
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/tests/fixtures/metrics_v1.prom");
    if std::env::var_os("MLXTOP_BLESS").is_some() {
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap();
    for (line, (actual, expected)) in actual.lines().zip(expected.lines()).enumerate() {
        assert_eq!(actual, expected, "metrics_v1.prom line {}", line + 1);
    }
    assert_eq!(actual.lines().count(), expected.lines().count());
}

#[test]
fn a_reading_the_snapshot_holds_is_written_with_its_value_and_unit() {
    let slowdown = values(&text(&mac_studio_slowdown(), Platform::MacOs));
    for (series, value) in [
        ("mlxtop_up", 1.0),
        ("mlxtop_sample_age_seconds", 0.25),
        ("mlxtop_interval_seconds", 1.0),
        ("mlxtop_ram_total_bytes", 38_654_705_664.0),
        ("mlxtop_ram_available_ratio", 0.24),
        ("mlxtop_memory_pressure_level", 0.0),
        ("mlxtop_memory_tight", 0.0),
        ("mlxtop_gpu_memory_limit_bytes", 29_360_128_000.0),
        ("mlxtop_swap_in_bytes_per_second", 6_291_456.0),
        ("mlxtop_swap_growth_bytes_per_second", 2_097_152.0),
        ("mlxtop_gpu_busy_ratio{gpu=\"0\"}", 0.96),
        ("mlxtop_gpu_tiler_busy_ratio{gpu=\"0\"}", 0.41),
        ("mlxtop_published_bandwidth_bytes_per_second", 410e9),
        ("mlxtop_thermal_pressure_level", 0.0),
        ("mlxtop_thermal_limited", 0.0),
        ("mlxtop_system_power_watts", 35.712),
        ("mlxtop_runtime_processes", 1.0),
        ("mlxtop_runtime_process_cpu_cores", 0.45),
        (
            "mlxtop_llm_model_info{provider=\"oMLX\",model=\"Qwen3.8-27B-oQ4e-mtp\"}",
            1.0,
        ),
        ("mlxtop_llm_telemetry_age_seconds", 0.25),
        ("mlxtop_llm_generation_tokens_per_second", 61.8),
        ("mlxtop_llm_prefill_average_tokens_per_second", 298.4),
        ("mlxtop_llm_requests_active", 1.0),
        ("mlxtop_llm_context_tokens", 41_023.0),
        ("mlxtop_llm_context_allowed_tokens", 131_072.0),
        ("mlxtop_llm_weight_bytes_per_pass", 15_020_479_488.0),
        ("mlxtop_llm_bits_per_weight", 4.691),
        ("mlxtop_llm_kv_bytes_per_token", 33_280.0),
        ("mlxtop_llm_state_bytes_per_sequence", 160_038_912.0),
        ("mlxtop_llm_context_trained_tokens", 262_144.0),
        ("mlxtop_llm_draft_weight_bytes", 3_848_808_960.0),
        ("mlxtop_llm_cache_reuse_ratio", 0.915),
        ("mlxtop_llm_prompt_tokens_total", 195_000_000.0),
        ("mlxtop_llm_cached_prompt_tokens_total", 178_425_000.0),
        ("mlxtop_llm_draft_acceptance_ratio", 0.78),
        ("mlxtop_llm_usual_generation_tokens_per_second", 123.0),
        ("mlxtop_llm_generation_slow_below_tokens_per_second", 98.4),
        ("mlxtop_llm_slowdown_seconds", 12.0),
        ("mlxtop_bottleneck{limiter=\"generation_slowed\"}", 1.0),
        ("mlxtop_bottleneck{limiter=\"healthy\"}", 0.0),
    ] {
        assert_eq!(slowdown.get(series), Some(&value), "{series}");
    }
    // A rate is written under the name of its kind only: this one is live.
    for absent in [
        "mlxtop_llm_generation_average_tokens_per_second",
        "mlxtop_llm_last_request_generation_tokens_per_second",
        "mlxtop_llm_prefill_tokens_per_second",
        "mlxtop_llm_usual_prefill_tokens_per_second",
        "mlxtop_llm_stall_seconds",
    ] {
        assert!(!slowdown.contains_key(absent), "{absent}");
    }
    // A stall says how long the answer has gone without a token.
    let stall = values(&text(&mac_studio_stall(), Platform::MacOs));
    assert_eq!(stall.get("mlxtop_llm_stall_seconds"), Some(&4.0));
    assert_eq!(
        stall.get("mlxtop_bottleneck{limiter=\"generation_stalled\"}"),
        Some(&1.0)
    );
    // A runtime read from its log gives the last request's speeds.
    let log = values(&text(&linux_workstation(), Platform::Linux));
    assert_eq!(
        log.get("mlxtop_llm_last_request_generation_tokens_per_second"),
        Some(&38.25)
    );
    assert_eq!(
        log.get("mlxtop_llm_last_request_prefill_tokens_per_second"),
        Some(&1890.0)
    );
    assert!(!log.contains_key("mlxtop_llm_generation_tokens_per_second"));
    // What Splash counts of its own work, and what mlxtop estimates from
    // it, served from a session that knows the best decode reached here.
    let splash = values(&served(&splash_slowdown()));
    for (series, value) in [
        (
            "mlxtop_llm_model_info{provider=\"Splash\",model=\"Qwen3.8-27B-oQ4e-mtp\"}",
            1.0,
        ),
        ("mlxtop_llm_tokens_per_verify_pass", 7.8),
        ("mlxtop_llm_kv_free_tokens", 110_528.0),
        ("mlxtop_llm_kv_free_bytes", 3_678_371_840.0),
        ("mlxtop_llm_preemptions_total", 2.0),
        ("mlxtop_llm_decode_verify_passes_per_second", 7.92),
        ("mlxtop_llm_decode_bytes_per_pass", 16_545_763_840.0),
        ("mlxtop_llm_decode_bandwidth_share_ratio", 0.3197),
        ("mlxtop_llm_decode_best_bandwidth_share_ratio", 0.6364),
        (
            "mlxtop_llm_decode_no_draft_ceiling_tokens_per_second",
            24.78,
        ),
        ("mlxtop_llm_context_left_tokens", 90_049.0),
        ("mlxtop_llm_context_left_requests", 1.0),
        ("mlxtop_llm_context_left_by_memory_tokens", 110_528.0),
        ("mlxtop_llm_context_left_by_window_tokens", 90_049.0),
    ] {
        assert_eq!(splash.get(series), Some(&value), "{series}");
    }
    // A snapshot with no session behind it knows no best.
    let once = values(&text(&splash_slowdown(), Platform::MacOs));
    assert!(once.contains_key("mlxtop_llm_decode_bandwidth_share_ratio"));
    assert!(!once.contains_key("mlxtop_llm_decode_best_bandwidth_share_ratio"));
}

#[test]
fn metric_names_follow_prometheus_conventions() {
    let written = text(&mac_studio_slowdown(), Platform::MacOs)
        + &text(&linux_workstation(), Platform::Linux)
        + &text(&mac_studio_stall(), Platform::MacOs)
        + &served(&splash_slowdown());
    let names: Vec<&str> = READINGS
        .iter()
        .map(|metric| metric.name)
        .chain(GPU_READINGS.iter().map(|metric| metric.name))
        .collect();
    let unique: BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(unique.len(), names.len(), "a name is used twice");
    for metric in READINGS {
        check_name(metric.name, metric.kind);
    }
    for metric in GPU_READINGS {
        check_name(metric.name, metric.kind);
    }
    // The table's helpers give each row its type.
    let probe: Metric<u8> = gauge("mlxtop_probe", "a probe", |reading| {
        Some(f64::from(*reading))
    });
    assert_eq!(
        (probe.name, probe.kind, probe.help),
        ("mlxtop_probe", GAUGE, "a probe")
    );
    assert_eq!((probe.value)(&3), Some(3.0));
    assert_eq!(
        counter::<u8>("mlxtop_probe_total", "a probe", |_| None).kind,
        COUNTER
    );
    // Every sample line names a family whose HELP and TYPE came first, and
    // carries only the labels the exporter means to write.
    let mut declared = BTreeSet::new();
    let mut helped = BTreeSet::new();
    for line in written.lines() {
        if let Some(rest) = line.strip_prefix("# HELP ") {
            helped.insert(rest.split(' ').next().unwrap().to_owned());
        } else if let Some(rest) = line.strip_prefix("# TYPE ") {
            let name = rest.split(' ').next().unwrap();
            assert!(helped.contains(name), "TYPE before HELP: {name}");
            declared.insert(name.to_owned());
        } else {
            let (series, value) = line.rsplit_once(' ').unwrap();
            assert!(value.parse::<f64>().unwrap().is_finite(), "{line}");
            let name = series.split('{').next().unwrap();
            assert!(declared.contains(name), "no TYPE for {line}");
            if let Some(labels) = series.strip_prefix(name) {
                for label in labels
                    .trim_matches(|c| c == '{' || c == '}')
                    .split(',')
                    .filter(|label| !label.is_empty())
                {
                    let key = label.split('=').next().unwrap();
                    assert!(
                        ["provider", "model", "limiter", "gpu"].contains(&key),
                        "label {key} in {line}"
                    );
                }
            }
        }
    }
}

/// A family name as Prometheus wants it: a prefix, a base unit, `_total`
/// on a counter and on nothing else.
fn check_name(name: &str, kind: &str) {
    assert!(name.starts_with("mlxtop_"), "{name}");
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'),
        "{name}"
    );
    assert_eq!(kind == COUNTER, name.ends_with("_total"), "{name}");
    for unit in [
        "percent", "milli", "_ms", "_kib", "_mib", "_gib", "_kb", "_mb", "_gb",
    ] {
        assert!(!name.contains(unit), "{name} is not in a base unit");
    }
    for suffix in ["_s", "_w", "_per_s", "_tok_per_s", "_mw"] {
        assert!(!name.ends_with(suffix), "{name} is not in a base unit");
    }
}

#[test]
fn unknown_readings_are_left_out_never_written_as_zero() {
    // The first sample of a session: nothing read yet.
    let text = text(&Sample::default(), Platform::MacOs);
    let values = values(&text);
    for (series, value) in &values {
        if *value == 0.0 {
            // The state set's other states, and a count of nothing found.
            assert!(
                series.starts_with("mlxtop_bottleneck{") || series == "mlxtop_runtime_processes",
                "{series} written as 0"
            );
        }
    }
    let written: BTreeSet<&str> = families(&text).into_iter().collect();
    assert_eq!(
        written,
        BTreeSet::from([
            "mlxtop_up",
            "mlxtop_sample_age_seconds",
            "mlxtop_interval_seconds",
            "mlxtop_bottleneck",
            "mlxtop_runtime_processes",
        ])
    );
    assert_eq!(
        values.get("mlxtop_bottleneck{limiter=\"collecting_baseline\"}"),
        Some(&1.0)
    );
    // With no runtime found, nothing is said about one.
    let quiet = self::text(&mac_no_runtime(), Platform::MacOs);
    assert!(!quiet.contains("mlxtop_llm_"), "{quiet}");
    assert!(quiet.contains("\nmlxtop_ram_total_bytes 38654705664\n"));
    // Linux has no compressor and reads no file paging: left out.
    let linux = self::text(&linux_workstation(), Platform::Linux);
    for absent in [
        "mlxtop_ram_compressed_bytes",
        "mlxtop_file_paging_in_bytes_per_second",
        "mlxtop_memory_compressed_bytes_per_second",
        "mlxtop_thermal_pressure_level",
        "mlxtop_gpu_memory_used_bytes",
        "mlxtop_published_bandwidth_bytes_per_second",
    ] {
        assert!(!linux.contains(absent), "{absent}");
    }
    // oMLX keeps Splash's counts private: nothing is written for them, nor
    // for the estimates that need them, even from a session.
    let private = served(&mac_studio_slowdown());
    for absent in [
        "mlxtop_llm_tokens_per_verify_pass",
        "mlxtop_llm_kv_free_",
        "mlxtop_llm_preemptions_total",
        "mlxtop_llm_decode_",
        "mlxtop_llm_context_left_",
    ] {
        assert!(!private.contains(absent), "{absent}");
    }
    // None preempted is a reading, written as 0.
    let mut none = splash_slowdown();
    none.llm_preemptions_total = Some(0);
    let none = self::values(&self::text(&none, Platform::MacOs));
    assert_eq!(none.get("mlxtop_llm_preemptions_total"), Some(&0.0));
    // Where only the window limits the context left, memory gives no figure.
    let mut sized = splash_slowdown();
    sized.llm_provider = "llama.cpp".into();
    let sized = self::values(&self::text(&sized, Platform::MacOs));
    assert!(!sized.contains_key("mlxtop_llm_context_left_by_memory_tokens"));
    assert_eq!(sized.get("mlxtop_llm_context_left_tokens"), Some(&90_049.0));
}

#[test]
fn each_gpu_is_labelled_with_its_index() {
    let values = values(&text(&linux_workstation(), Platform::Linux));
    for (series, value) in [
        ("mlxtop_gpu_busy_ratio{gpu=\"0\"}", 0.91),
        ("mlxtop_gpu_busy_ratio{gpu=\"1\"}", 0.12),
        (
            "mlxtop_gpu_vram_used_bytes{gpu=\"0\"}",
            20.0 * 1024.0 * 1024.0 * 1024.0,
        ),
        (
            "mlxtop_gpu_vram_total_bytes{gpu=\"1\"}",
            48.0 * 1024.0 * 1024.0 * 1024.0,
        ),
        ("mlxtop_gpu_temperature_celsius{gpu=\"0\"}", 71.0),
        ("mlxtop_gpu_throttled{gpu=\"0\"}", 1.0),
        ("mlxtop_vram_total_bytes", 72.0 * 1024.0 * 1024.0 * 1024.0),
        ("mlxtop_hottest_temperature_celsius", 71.0),
    ] {
        assert_eq!(values.get(series), Some(&value), "{series}");
    }
    // A driver that does not say why clocks are held is unknown.
    assert!(!values.contains_key("mlxtop_gpu_throttled{gpu=\"1\"}"));
    assert!(!values.contains_key("mlxtop_gpu_renderer_busy_ratio{gpu=\"0\"}"));
    // One HELP and TYPE a family, however many cards.
    let text = text(&linux_workstation(), Platform::Linux);
    assert_eq!(
        text.matches("# TYPE mlxtop_gpu_busy_ratio gauge").count(),
        1
    );
    // A card that is no longer held back says 0.
    let mut free = linux_workstation();
    free.gpus[0].throttle_reasons = Some(0x1);
    let free = self::values(&self::text(&free, Platform::Linux));
    assert_eq!(free.get("mlxtop_gpu_throttled{gpu=\"0\"}"), Some(&0.0));
}

#[test]
fn the_verdict_is_a_state_set_with_one_state_on() {
    let names: Vec<String> = verdicts().map(state_name).collect();
    let distinct: BTreeSet<&String> = names.iter().collect();
    assert_eq!(names.len(), 20);
    assert_eq!(distinct.len(), names.len());
    let mut taken = taken(&mac_studio_slowdown(), Platform::MacOs);
    for verdict in verdicts() {
        taken.snapshot.verdict.state = verdict;
        let text = metrics(Some(&taken), taken.at);
        let states: Vec<(String, f64)> = values(&text)
            .into_iter()
            .filter(|(series, _)| series.starts_with("mlxtop_bottleneck{"))
            .collect();
        assert_eq!(states.len(), names.len());
        let on: Vec<&String> = states
            .iter()
            .filter(|(_, value)| *value == 1.0)
            .map(|(series, _)| series)
            .collect();
        assert_eq!(
            on,
            [&format!(
                "mlxtop_bottleneck{{limiter=\"{}\"}}",
                state_name(verdict)
            )]
        );
    }
}

#[test]
fn label_values_are_escaped() {
    let mut sample = mac_studio_slowdown();
    sample.llm_model = "a \"quoted\" \\ model\nname".into();
    let text = text(&sample, Platform::MacOs);
    assert!(
        text.contains(r#"model="a \"quoted\" \\ model\nname"} 1"#),
        "{text}"
    );
    // With no model named, the runtime is still said.
    sample.llm_model = "not detected".into();
    let text = self::text(&sample, Platform::MacOs);
    assert!(text.contains("mlxtop_llm_model_info{provider=\"oMLX\",model=\"\"} 1"));
}

#[test]
fn while_sampling_is_not_current_only_up_and_the_age_are_written() {
    let up = "# HELP mlxtop_up 1 while sampling is current (the newest sample at most three intervals old, 5 s at least); 0 otherwise, and then no reading is written\n# TYPE mlxtop_up gauge\n";
    // Before the first sample.
    assert_eq!(metrics(None, Instant::now()), format!("{up}mlxtop_up 0\n"));
    // Three intervals and 5 s at least: at 1 s a sample, current until 5 s.
    let taken = taken(&mac_studio_slowdown(), Platform::MacOs);
    let at = |seconds: u64| metrics(Some(&taken), taken.at + Duration::from_secs(seconds));
    assert!(at(5).contains("\nmlxtop_up 1\n"));
    let stale = at(6);
    assert_eq!(
        stale,
        format!("{up}mlxtop_up 0\n# HELP mlxtop_sample_age_seconds Seconds since the newest sample was taken\n# TYPE mlxtop_sample_age_seconds gauge\nmlxtop_sample_age_seconds 6\n")
    );
    // At a 10 s interval, a sample is current for 30 s.
    let mut slow = taken;
    slow.snapshot.interval_s = 10;
    let at = |seconds: u64| metrics(Some(&slow), slow.at + Duration::from_secs(seconds));
    assert!(at(30).contains("\nmlxtop_up 1\n"));
    assert!(at(31).contains("\nmlxtop_up 0\n"));
}

#[test]
fn the_user_guide_lists_every_metric() {
    let guide = include_str!("../../docs/USER_GUIDE.md");
    let missing: Vec<&str> = READINGS
        .iter()
        .map(|metric| metric.name)
        .chain(GPU_READINGS.iter().map(|metric| metric.name))
        .chain([
            "mlxtop_up",
            "mlxtop_sample_age_seconds",
            "mlxtop_bottleneck",
            "mlxtop_llm_model_info",
        ])
        .filter(|name| !guide.contains(&format!("`{name}`")))
        .collect();
    assert!(missing.is_empty(), "not in the user guide: {missing:?}");
    // Every family the fixtures write is one of those.
    for platform_text in [
        text(&mac_studio_slowdown(), Platform::MacOs),
        text(&linux_workstation(), Platform::Linux),
        served(&splash_slowdown()),
    ] {
        for name in families(&platform_text) {
            assert!(guide.contains(&format!("`{name}`")), "{name}");
        }
    }
}

// --- requests -----------------------------------------------------------------

fn request(method: &str, path: &str, host: &str) -> Vec<u8> {
    format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: test\r\nAccept: */*")
        .into_bytes()
}

fn get(path: &str) -> Vec<u8> {
    request("GET", path, "127.0.0.1:10063")
}

fn answer_text(reply: &Reply) -> String {
    String::from_utf8(reply.bytes()).unwrap()
}

#[test]
fn each_path_answers_with_its_own_content_type() {
    let taken = taken(&mac_studio_slowdown(), Platform::MacOs);
    let now = taken.at + Duration::from_millis(250);
    let metrics = respond(&get("/metrics"), Some(&taken), now);
    assert_eq!(metrics.status, Status::Ok);
    assert_eq!(
        metrics.content_type,
        "text/plain; version=0.0.4; charset=utf-8"
    );
    assert_eq!(
        String::from_utf8(metrics.body.clone()).unwrap(),
        super::metrics(Some(&taken), now)
    );
    // The snapshot is the line `--once --json` prints.
    let snapshot = respond(&get("/snapshot"), Some(&taken), now);
    assert_eq!(snapshot.status, Status::Ok);
    assert_eq!(snapshot.content_type, "application/json");
    let mut line = Vec::new();
    crate::export::write_line(&mut line, &taken.snapshot).unwrap();
    assert_eq!(snapshot.body, line);
    let value: Value = serde_json::from_slice(&snapshot.body).unwrap();
    assert_eq!(value["schema_version"], crate::export::SCHEMA_VERSION);
    let health = respond(&get("/health"), Some(&taken), now);
    assert_eq!(
        (health.status, health.body.as_slice()),
        (Status::Ok, &b"ok\n"[..])
    );
    assert_eq!(health.content_type, "text/plain; charset=utf-8");
    // A query is ignored.
    assert_eq!(
        respond(&get("/metrics?name=x"), Some(&taken), now).status,
        Status::Ok
    );
    // Sampling that fell behind, and none yet.
    let late = respond(
        &get("/health"),
        Some(&taken),
        taken.at + Duration::from_secs(12),
    );
    assert_eq!(late.status, Status::Unavailable);
    assert_eq!(late.body, b"no sample for 12s\n");
    for path in ["/health", "/snapshot"] {
        let none = respond(&get(path), None, now);
        assert_eq!(
            (none.status, none.body.as_slice()),
            (Status::Unavailable, &b"no sample yet\n"[..])
        );
    }
    // /metrics always answers: mlxtop_up says whether it is current.
    let none = respond(&get("/metrics"), None, now);
    assert_eq!(none.status, Status::Ok);
    for path in ["/", "/metrics/", "/index.html", "/api/v1/query"] {
        assert_eq!(
            respond(&get(path), Some(&taken), now).status,
            Status::NotFound,
            "{path}"
        );
    }
}

#[test]
fn only_get_and_head_are_served_with_no_cors_and_no_html() {
    let taken = taken(&mac_studio_slowdown(), Platform::MacOs);
    let now = taken.at;
    for method in [
        "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "TRACE", "CONNECT",
    ] {
        let reply = respond(&request(method, "/metrics", "localhost"), Some(&taken), now);
        assert_eq!(reply.status, Status::MethodNotAllowed, "{method}");
        assert!(answer_text(&reply).contains("\r\nAllow: GET, HEAD\r\n"));
    }
    // HEAD: a GET's headers, its length included, and no body.
    let got = respond(&get("/metrics"), Some(&taken), now);
    let head = respond(&request("HEAD", "/metrics", "localhost"), Some(&taken), now);
    let head = answer_text(&head);
    assert!(head.ends_with("\r\n\r\n"), "{head}");
    assert!(head.contains(&format!("\r\nContent-Length: {}\r\n", got.body.len())));
    // Not a header to share across origins, never a page to render.
    for reply in [
        got,
        respond(&get("/snapshot"), Some(&taken), now),
        respond(&get("/health"), Some(&taken), now),
        respond(&get("/nope"), Some(&taken), now),
        respond(
            &request("OPTIONS", "/metrics", "localhost"),
            Some(&taken),
            now,
        ),
    ] {
        let text = answer_text(&reply).to_ascii_lowercase();
        assert!(!text.contains("access-control"), "{text}");
        assert!(!text.contains("text/html"), "{text}");
        assert!(text.contains("\r\nx-content-type-options: nosniff\r\n"));
        assert!(text.contains("\r\nconnection: close\r\n"));
    }
}

#[test]
fn a_host_header_that_is_not_loopback_is_refused() {
    for host in [
        "localhost",
        "localhost:10063",
        "LocalHost:1",
        "127.0.0.1",
        "127.0.0.1:10063",
        "127.4.5.6:80",
        "[::1]",
        "[::1]:19063",
        "[::ffff:127.0.0.1]:10063",
    ] {
        assert!(loopback_host(host), "{host}");
    }
    for host in [
        "",
        "evil.example",
        "evil.example:10063",
        "localhost.evil.example",
        "127.0.0.1.evil.example:10063",
        "localhost:",
        "localhost:http",
        "::1",
        "[::1",
        "[::1]10063",
        "[::1]:",
        "[::]:10063",
        "0.0.0.0:10063",
        "192.168.50.132:10063",
        "2130706433",
    ] {
        assert!(!loopback_host(host), "{host}");
    }
    let taken = taken(&mac_studio_slowdown(), Platform::MacOs);
    let foreign = respond(
        &request("GET", "/metrics", "rebound.example:10063"),
        Some(&taken),
        taken.at,
    );
    assert_eq!(foreign.status, Status::Forbidden);
    assert!(!answer_text(&foreign).contains("mlxtop_up"));
    // HEAD is refused the same way, with no body.
    let head = respond(
        &request("HEAD", "/metrics", "rebound.example"),
        Some(&taken),
        taken.at,
    );
    assert_eq!(head.status, Status::Forbidden);
    assert!(answer_text(&head).ends_with("\r\n\r\n"));
    // A request must name one host: none, or two, is not one.
    let none = b"GET /metrics HTTP/1.1\r\nAccept: */*".to_vec();
    let two = b"GET /metrics HTTP/1.1\r\nHost: localhost\r\nhost: evil.example".to_vec();
    for head in [none, two] {
        assert_eq!(
            respond(&head, Some(&taken), taken.at).status,
            Status::BadRequest
        );
    }
    // The header's name is not case-sensitive.
    let upper = b"GET /health HTTP/1.0\r\nHOST:  127.0.0.1 ".to_vec();
    assert_eq!(respond(&upper, Some(&taken), taken.at).status, Status::Ok);
}

#[test]
fn a_head_that_is_not_an_http_1_request_is_refused() {
    for head in [
        &b"GARBAGE"[..],
        b"GET /metrics",
        b"GET /metrics HTTP/2.0\r\nHost: localhost",
        b"GET  /metrics HTTP/1.1\r\nHost: localhost",
        b"GET /metrics HTTP/1.1 extra\r\nHost: localhost",
        b"GET /metrics HTTP/1.1\r\nHost localhost",
        b"GET /metrics HTTP/1.1\r\nHost: localhost\r\n folded",
        b"GET /metrics HTTP/1.1\r\nBad Name: x\r\nHost: localhost",
        b"GET /metrics HTTP/1.1\r\n: x\r\nHost: localhost",
        b"GET /metrics HTTP/1.1\r\nHost: \xff",
        b"",
    ] {
        assert_eq!(
            respond(head, None, Instant::now()).status,
            Status::BadRequest,
            "{}",
            String::from_utf8_lossy(head)
        );
    }
    let refused = answer_text(&respond(b"GARBAGE", None, Instant::now()));
    assert!(
        refused.starts_with("HTTP/1.1 400 Bad Request\r\n"),
        "{refused}"
    );
    assert!(
        refused.ends_with("\r\n\r\nnot an HTTP/1 request\n"),
        "{refused}"
    );
}

// --- the listener over loopback ---------------------------------------------

/// A listener answering from `latest` on a loopback port, until dropped.
struct Listening {
    port: u16,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<io::Result<()>>>,
}

impl Listening {
    fn new(latest: Latest) -> Self {
        let listener = bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = thread::spawn({
            let stop = stop.clone();
            move || answer_all(listener, &latest, &stop)
        });
        Self {
            port,
            stop,
            handle: Some(handle),
        }
    }

    fn connect(&self) -> TcpStream {
        let stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
    }

    /// Send `request` whole and read the answer to its end.
    fn ask(&self, request: &[u8]) -> String {
        let mut stream = self.connect();
        stream.write_all(request).unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        reply
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap().unwrap();
        }
    }
}

fn published() -> Latest {
    let latest = Latest::default();
    latest.publish(crate::export::snapshot(
        &mac_studio_slowdown(),
        Platform::MacOs,
        1,
    ));
    latest
}

fn whole_request(path: &str) -> Vec<u8> {
    let mut bytes = get(path);
    bytes.extend_from_slice(b"\r\n\r\n");
    bytes
}

/// An answer's status line, headers by lowercase name, and body.
fn parts(reply: &str) -> (&str, BTreeMap<String, String>, &str) {
    let (head, body) = reply.split_once("\r\n\r\n").unwrap();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap();
    let headers = lines
        .map(|line| {
            let (name, value) = line.split_once(": ").unwrap();
            (name.to_ascii_lowercase(), value.to_owned())
        })
        .collect();
    (status, headers, body)
}

#[test]
fn a_scrape_over_loopback_gets_the_prometheus_content_type() {
    let listening = Listening::new(published());
    let reply = listening.ask(&whole_request("/metrics"));
    let (status, headers, body) = parts(&reply);
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert_eq!(
        headers["content-type"],
        "text/plain; version=0.0.4; charset=utf-8"
    );
    assert_eq!(headers["content-length"], body.len().to_string());
    assert_eq!(headers["connection"], "close");
    assert!(body.contains("\nmlxtop_up 1\n"), "{body}");
    assert!(body.contains("mlxtop_llm_model_info{provider=\"oMLX\""));
    // HEAD over the wire: the length, and no body.
    let head = listening.ask(b"HEAD /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n");
    let (status, headers, body) = parts(&head);
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(headers["content-length"].parse::<usize>().unwrap() > 1_000);
    assert_eq!(body, "");
    // A foreign host over the wire.
    let foreign = listening.ask(b"GET /metrics HTTP/1.1\r\nHost: evil.example\r\n\r\n");
    assert!(
        foreign.starts_with("HTTP/1.1 403 Forbidden\r\n"),
        "{foreign}"
    );
    // The blank line may arrive in pieces.
    let mut stream = listening.connect();
    stream.write_all(&get("/health")).unwrap();
    stream.write_all(b"\r\n").unwrap();
    thread::sleep(Duration::from_millis(100));
    stream.write_all(b"\r\n").unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    assert!(reply.ends_with("\r\n\r\nok\n"), "{reply}");
    // A client that connects and leaves costs nothing.
    drop(listening.connect());
    assert!(listening
        .ask(&whole_request("/health"))
        .starts_with("HTTP/1.1 200 OK"));
}

#[test]
fn a_client_that_stalls_is_released_after_two_seconds() {
    let listening = Listening::new(published());
    let mut stalled = listening.connect();
    stalled
        .write_all(b"GET /metrics HTTP/1.1\r\nHost: 127.0.0.1")
        .unwrap();
    let started = Instant::now();
    let mut reply = String::new();
    stalled.read_to_string(&mut reply).unwrap();
    let waited = started.elapsed();
    assert!(
        reply.starts_with("HTTP/1.1 408 Request Timeout\r\n"),
        "{reply}"
    );
    assert!(
        (Duration::from_millis(1_800)..Duration::from_secs(4)).contains(&waited),
        "released after {waited:?}"
    );
    // The listener is free again at once.
    let started = Instant::now();
    assert!(listening
        .ask(&whole_request("/health"))
        .starts_with("HTTP/1.1 200 OK"));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn a_client_that_trickles_gets_no_longer_than_one_that_stalls() {
    let listening = Listening::new(published());
    let mut trickle = listening.connect();
    let started = Instant::now();
    // Each byte comes in time for a read; the head as a whole does not.
    for byte in b"GET /me" {
        trickle.write_all(&[*byte]).unwrap();
        thread::sleep(Duration::from_millis(250));
    }
    let mut reply = String::new();
    trickle.read_to_string(&mut reply).unwrap();
    let waited = started.elapsed();
    assert!(
        reply.starts_with("HTTP/1.1 408 Request Timeout\r\n"),
        "{reply}"
    );
    assert!(
        waited < Duration::from_millis(3_000),
        "released after {waited:?}"
    );
}

#[test]
fn a_request_head_is_8_kib_at_most() {
    let listening = Listening::new(published());
    // Exactly 8 KiB, blank line included, is still a request.
    let mut head = get("/health");
    head.extend_from_slice(b"\r\nX-Pad: ");
    head.resize(8 * 1024 - 4, b'a');
    head.extend_from_slice(b"\r\n\r\n");
    assert_eq!(head.len(), 8 * 1024);
    assert!(listening.ask(&head).starts_with("HTTP/1.1 200 OK"));
    // 8 KiB with no end in sight is not.
    let reply = listening.ask(&vec![b'a'; 8 * 1024]);
    assert!(
        reply.starts_with("HTTP/1.1 431 Request Header Fields Too Large\r\n"),
        "{reply}"
    );
}

// --- --listen and serving ---------------------------------------------------

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

fn serve_action(listen: &str) -> CliAction {
    CliAction::Serve {
        interval: 1,
        history: 300,
        record: None,
        listen: listen.parse().unwrap(),
    }
}

#[test]
fn serve_listens_on_loopback_only() {
    let config = Config::default();
    let parse = |words: &[&str]| parse_args(&args(words), &config);
    assert_eq!(parse(&["serve"]).unwrap(), serve_action("127.0.0.1:10063"));
    for (listen, address) in [
        ("127.0.0.1:9999", "127.0.0.1:9999"),
        ("[::1]:10063", "[::1]:10063"),
        ("localhost:10064", "127.0.0.1:10064"),
        ("127.0.0.2:0", "127.0.0.2:0"),
    ] {
        assert_eq!(
            parse(&["serve", "--listen", listen]).unwrap(),
            serve_action(address),
            "{listen}"
        );
    }
    assert_eq!(
        parse(&["serve", "-i", "5", "--record", "s.jsonl"]).unwrap(),
        CliAction::Serve {
            interval: 5,
            history: 300,
            record: Some("s.jsonl".into()),
            listen: DEFAULT_LISTEN,
        }
    );
    for address in ["0.0.0.0:10063", "192.168.50.132:10063", "[::]:10063"] {
        let error = parse(&["serve", "--listen", address])
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with("serve listens on loopback only"),
            "{error}"
        );
        assert!(error.contains("ssh -L 10063:127.0.0.1:10063"), "{error}");
    }
    for (input, error) in [
        (
            &["serve", "--listen", "mac-studio.local:10063"][..],
            "--listen takes a loopback address and a port, as 127.0.0.1:10063 (got \"mac-studio.local:10063\")",
        ),
        (
            &["serve", "--listen", "10063"][..],
            "--listen takes a loopback address and a port, as 127.0.0.1:10063 (got \"10063\")",
        ),
        (
            &["serve", "--listen", "localhost:99999"][..],
            "--listen takes a loopback address and a port, as 127.0.0.1:10063 (got \"localhost:99999\")",
        ),
        (&["serve", "--listen"][..], "missing listen address"),
        (
            &["serve", "--once"][..],
            "serve cannot be combined with --once or --json",
        ),
        (
            &["serve", "--json"][..],
            "serve cannot be combined with --once or --json",
        ),
        (
            &["--listen", "127.0.0.1:10063"][..],
            "--listen goes with serve",
        ),
        (
            &["stream", "--listen", "127.0.0.1:10063"][..],
            "--listen goes with serve",
        ),
        (
            &["doctor", "--listen", "127.0.0.1:10063"][..],
            "--listen goes with serve",
        ),
        (&["--once", "serve"][..], "unknown option: serve"),
    ] {
        assert_eq!(parse(input).unwrap_err().to_string(), error, "{input:?}");
    }
    // The listener itself refuses anything else too.
    let refused = bind("0.0.0.0:0".parse().unwrap()).unwrap_err();
    assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);
    // Help names the command and the way in from another machine, in 80 columns.
    let help = help_text(Platform::MacOs);
    for words in [
        "mlxtop serve [--listen ADDR] [--interval N]",
        "ssh -L 10063:127.0.0.1:10063 HOST",
        "--listen ADDR     serve's loopback address:port (default 127.0.0.1:10063)",
    ] {
        let line = help
            .lines()
            .find(|line| line.contains(words))
            .unwrap_or_else(|| panic!("help lacks {words}"));
        assert!(line.chars().count() <= 80, "{line}");
    }
}

/// A collector on a recorded Linux host, its runtime port closed.
fn build(host: FakeHost) -> impl FnOnce() -> Collector + Send + 'static {
    move || Collector::with_host(60, offline_config(), Box::new(host), Platform::Linux, None)
}

#[test]
fn the_headless_sampler_fills_the_slot_and_builds_no_view() {
    let latest = Latest::default();
    let sampler = Sampler::headless(
        Duration::from_millis(10),
        build(linux_host()),
        latest.clone(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let first = loop {
        if let Some(taken) = latest.get() {
            break taken;
        }
        assert!(Instant::now() < deadline, "no sample");
        thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(first.snapshot.host.platform, "linux");
    // The next sample replaces it.
    while latest
        .get()
        .is_some_and(|taken| Arc::ptr_eq(&taken, &first))
    {
        assert!(Instant::now() < deadline, "no second sample");
        thread::sleep(Duration::from_millis(5));
    }
    assert!(latest.get().unwrap().at > first.at);
    assert!(sampler.views.try_recv().is_err(), "a view was built");
    drop(sampler);
}

#[test]
fn serve_answers_from_the_latest_sample_and_never_samples_for_a_request() {
    let host = linux_host();
    let watched = host.clone();
    let listener = bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let port = listener.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let serving = thread::spawn({
        let stop = stop.clone();
        move || {
            serve(listener, Duration::from_secs(3600), build(host), &|| {
                stop.load(Ordering::SeqCst)
            })
        }
    });
    let ask = |path: &str| {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream.write_all(&whole_request(path)).unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        reply
    };
    // The first sample arrives; until then health says so.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ask("/health").starts_with("HTTP/1.1 200 OK") {
        assert!(Instant::now() < deadline, "no first sample");
        thread::sleep(Duration::from_millis(20));
    }
    let sampled = watched.calls().len();
    for _ in 0..5 {
        let metrics = ask("/metrics");
        assert!(metrics.contains("\nmlxtop_up 1\n"), "{metrics}");
        let snapshot = ask("/snapshot");
        let (_, _, body) = parts(&snapshot);
        let value: Value = serde_json::from_str(body).unwrap();
        assert_eq!(value["host"]["platform"], "linux");
    }
    assert_eq!(watched.calls().len(), sampled, "a request read the host");
    // A stop ends serving cleanly, between requests.
    let stopping = Instant::now();
    stop.store(true, Ordering::SeqCst);
    serving.join().unwrap().unwrap();
    assert!(stopping.elapsed() < Duration::from_secs(2));
    assert!(
        TcpStream::connect(("127.0.0.1", port)).is_err(),
        "still listening"
    );
}

#[test]
fn serving_ends_with_an_error_when_sampling_stops() {
    let host = linux_host().panic_on("/bin/date +%H:%M:%S %z");
    let listener = bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let ended = serve(listener, Duration::from_secs(1), build(host), &|| false).unwrap_err();
    assert_eq!(ended.to_string(), "sampling stopped; the log says why");
}
