// SPDX-License-Identifier: MIT
use super::*;

#[test]
fn connection_status_and_retry_preserve_success_age() {
    let mut report = RuntimeReport {
        provider: Some("Ollama".into()),
        ..RuntimeReport::default()
    };
    assert_eq!(report.status(), "Awaiting sample");
    report.begin();
    report.record::<()>("/api/ps", Err(ProbeIssue::Http(401)));
    report.finish(false);
    assert!(report.failed());
    report.record("/api/ps", Ok(()));
    report.finish(true);
    assert_eq!(report.status(), "Connected");
    assert!(!report.failed());
    let at = report.succeeded_at;
    report.begin();
    report.record::<()>("/api/ps", Err(ProbeIssue::Timeout));
    report.finish(false);
    assert_eq!(report.succeeded_at, at);
    assert!(report.failed());
}

#[test]
fn capabilities_preserve_sources_and_missing_or_stale_observations() {
    for name in [
        "oMLX",
        "llama.cpp",
        "KoboldCpp",
        "Ollama",
        "LM Studio",
        "vLLM",
        "SGLang",
        "mlx-lm",
        "LocalAI",
        "Jan",
        "GPT4All",
        "TensorFold",
        "Strata",
    ] {
        let capabilities = capabilities(name);
        assert_eq!(capabilities.len(), 7, "{name}");
        for capability in capabilities {
            assert!(matches!(
                capability.observed(&Sample::default()).as_str(),
                "not observed" | "unavailable"
            ));
        }
        assert!(!setup(name).is_empty());
        assert!(default_port(Some(name)) > 0);
    }
    assert!(capabilities("unsupported").is_empty());
    let mut sample = Sample {
        llm_source: TelemetrySource::Live,
        llm_model: "test".into(),
        llm_generation_tps: Some(42.0),
        llm_prefill_tps: Some(100.0),
        llm_active_requests: Some(0),
        llm_cache_efficiency: Some(50.0),
        llm_observed_at: Some(SystemTime::now()),
        ..Sample::default()
    };
    sample.llm_requests.push(crate::domain::RequestUsage {
        provider: "oMLX".into(),
        model: "test".into(),
        id: "r1".into(),
        prompt: 10,
        cached: Some(5),
        output: Some(3),
        output_tps: None,
        completed: true,
        queued: false,
        prefilled: None,
        ttft_ms: Some(42),
        observed_at: sample.llm_observed_at,
    });
    for c in capabilities("oMLX") {
        assert!(!c.observed(&sample).contains("not observed"));
    }
    sample.llm_status = "stale".into();
    for c in capabilities("oMLX") {
        assert!(
            c.observed(&sample).contains("stale")
                || c.observed(&sample).contains("reported completions")
        );
    }
    sample.llm_status = "running".into();
    sample.llm_observed_at = Some(SystemTime::now() - std::time::Duration::from_secs(60));
    assert!(capabilities("oMLX")[1].observed(&sample).contains("stale"));
    sample.llm_source = TelemetrySource::Report;
    assert_eq!(capabilities("Ollama")[1].source, "client recorder");
    assert!(capabilities("Ollama")[1]
        .observed(&sample)
        .contains("LAST result"));
}

#[test]
fn diagnostics_are_read_only_sanitized_and_useful_without_a_runtime() {
    use crate::test_support::TempDir;
    let mut report = RuntimeReport::default();
    report.begin();
    report.finish(false);
    assert!(!report.failed());
    assert_eq!(report.status(), "No runtime detected");
    assert!(report
        .lines(&Sample::default())
        .join("\n")
        .contains("MLXTOP_PROVIDER=ollama mlxtop doctor"));
    report.provider = Some("LM Studio".into());
    report.finish(true);
    assert_eq!(report.status(), "Connected · partial availability");
    report.remote = true;
    report.credentials_present = true;
    let text = report.lines(&Sample::default()).join("\n");
    assert!(text.contains("configured (hidden)"));
    assert!(text.contains("host counters are local"));
    assert!(text.contains("This variable alone does not record requests"));
    for value in [
        "http://user:secret@host",
        "https://host?key=secret",
        "https://host/#secret",
        "invalid",
        "http://host/\u{1b}[31m",
    ] {
        assert_eq!(safe_endpoint(value), None);
    }
    assert_eq!(
        safe_endpoint("https://example.net/proxy"),
        Some("https://example.net/proxy".into())
    );
    let dir = TempDir::new("runtime-usage");
    report.usage_file(None, false);
    assert_eq!(report.usage, "not configured");
    report.usage_file(Some(&dir.0.join("absent")), false);
    assert!(report.usage.contains("does not exist"));
    report.usage_file(Some(&dir.0), false);
    assert!(report.usage.contains("readable file"));
    let path = dir.write("usage.jsonl", "invalid\n");
    report.usage_file(Some(&path), false);
    assert!(report.usage.contains("no valid"));
    report.usage_file(Some(&path), true);
    assert!(report.usage.contains("available"));
}

#[test]
fn safe_error_messages_preserve_actionable_failure_categories() {
    for issue in [
        ProbeIssue::InvalidEndpoint,
        ProbeIssue::InvalidProvider,
        ProbeIssue::InvalidPort,
        ProbeIssue::InvalidCredential,
        ProbeIssue::RemoteAuthBlocked,
        ProbeIssue::CredentialsMissing,
        ProbeIssue::Timeout,
        ProbeIssue::Transport,
        ProbeIssue::Http(401),
        ProbeIssue::Http(403),
        ProbeIssue::Http(404),
        ProbeIssue::InvalidResponse,
        ProbeIssue::ResponseTooLarge,
    ] {
        assert!(!issue.to_string().is_empty());
    }
    assert_eq!(
        ProbeIssue::from_io(std::io::ErrorKind::TimedOut.into()),
        ProbeIssue::Timeout
    );
    assert_eq!(
        ProbeIssue::from_io(std::io::ErrorKind::ConnectionRefused.into()),
        ProbeIssue::Transport
    );
}

#[test]
fn capability_freshness_belongs_to_its_source() {
    let mut sample = Sample {
        llm_source: TelemetrySource::Live,
        llm_status: "loaded".into(),
        llm_model: "model".into(),
        llm_generation_tps: Some(10.0),
        llm_generation_tps_live: true,
        llm_observed_at: Some(SystemTime::now()),
        ..Default::default()
    };
    assert!(capabilities("oMLX")[1]
        .observed(&sample)
        .starts_with("LIVE"));
    sample.llm_generation_tps_live = false;
    assert!(capabilities("llama.cpp")[1]
        .observed(&sample)
        .starts_with("SERVER AVG"));
    assert_eq!(
        capabilities("LM Studio")[0].observed(&sample),
        "loaded inventory"
    );
    sample.llm_status = "available".into();
    assert_eq!(
        capabilities("LM Studio")[0].observed(&sample),
        "available-model catalogue"
    );
    sample.llm_status = "no models".into();
    assert_eq!(
        capabilities("LM Studio")[0].observed(&sample),
        "empty inventory"
    );
    sample.llm_requests.push(crate::domain::RequestUsage {
        provider: "Ollama".into(),
        model: "model".into(),
        id: "completed".into(),
        prompt: 100,
        cached: Some(40),
        output: None,
        output_tps: None,
        completed: true,
        queued: false,
        prefilled: None,
        ttft_ms: Some(100),
        observed_at: Some(SystemTime::now() - std::time::Duration::from_secs(60)),
    });
    let rows = capabilities("Ollama");
    for index in [4, 5, 6] {
        let text = rows[index].observed(&sample);
        assert!(
            text.contains("reported completions") && text.contains("1m old"),
            "{text}"
        );
    }
    sample.llm_requests[0].completed = false;
    assert!(rows[4]
        .observed(&sample)
        .contains("sampled request observations"));
    sample.llm_generation_tps = None;
    sample.llm_requests[0].completed = true;
    sample.llm_requests[0].output_tps = Some(12.0);
    assert_eq!(
        rows[1].observed(&sample),
        "reported completions · newest 1m old"
    );
}
