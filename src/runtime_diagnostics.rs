// SPDX-License-Identifier: MIT
//! Shared, credential-free connection observations and capability guidance.
use crate::domain::{Sample, TelemetrySource};
use crate::formatting::telemetry_age;
use std::fmt;
use std::path::Path;
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProbeIssue {
    InvalidEndpoint,
    InvalidProvider,
    InvalidPort,
    InvalidCredential,
    RemoteAuthBlocked,
    CredentialsMissing,
    Timeout,
    Transport,
    Http(u16),
    InvalidResponse,
    ResponseTooLarge,
}

impl fmt::Display for ProbeIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEndpoint => f.write_str("invalid endpoint; use an HTTP(S) server root without credentials, query or fragment"),
            Self::InvalidProvider => f.write_str("unsupported MLXTOP_PROVIDER; choose a documented provider name"),
            Self::InvalidPort => f.write_str("invalid MLXTOP_PROVIDER_PORT; use an integer from 1 to 65535"),
            Self::InvalidCredential => f.write_str("invalid credential; remove empty values and control characters"),
            Self::RemoteAuthBlocked => f.write_str("remote authentication blocked; MLXTOP_ALLOW_REMOTE_AUTH=1 is required to send credentials"),
            Self::CredentialsMissing => f.write_str("monitoring credentials not configured"),
            Self::Timeout => f.write_str("connection timed out; check the server address and reachability"),
            Self::Transport => f.write_str("connection failed; check reachability and HTTPS certificate trust"),
            Self::Http(401 | 403) => f.write_str("authentication rejected; check monitoring credentials and permissions"),
            Self::Http(status) => write!(f, "HTTP {status}; check that this monitoring endpoint is enabled"),
            Self::InvalidResponse => f.write_str("unrecognized response; check the endpoint and runtime API version"),
            Self::ResponseTooLarge => f.write_str("response exceeds the monitoring size limit"),
        }
    }
}

impl ProbeIssue {
    pub(crate) fn from_io(error: std::io::Error) -> Self {
        match error.kind() {
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => Self::Timeout,
            _ => Self::Transport,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Probe {
    pub path: String,
    pub issue: Option<ProbeIssue>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RuntimeReport {
    pub provider: Option<String>,
    pub selection: &'static str,
    pub endpoint: String,
    pub remote: bool,
    pub credentials_present: bool,
    pub attempted_at: Option<SystemTime>,
    pub succeeded_at: Option<SystemTime>,
    pub connected: bool,
    pub recorder_only: bool,
    pub probes: Vec<Probe>,
    pub usage: &'static str,
}

impl RuntimeReport {
    pub(crate) fn begin(&mut self) {
        self.attempted_at = Some(SystemTime::now());
        self.connected = false;
        self.probes.clear();
    }

    pub(crate) fn record<T>(&mut self, path: &str, result: Result<T, ProbeIssue>) -> Option<T> {
        // A retry replaces an earlier failure for this endpoint, rather than
        // leaving a repaired authentication session marked as failed.
        self.probes.retain(|probe| probe.path != path);
        self.probes.push(Probe {
            path: path.into(),
            issue: result.as_ref().err().cloned(),
        });
        result.ok()
    }

    pub(crate) fn finish(&mut self, connected: bool) {
        self.connected = connected;
        if connected {
            self.succeeded_at = Some(SystemTime::now());
        } else if !self.probes.iter().any(|probe| probe.issue.is_some()) {
            self.record::<()>("telemetry", Err(ProbeIssue::InvalidResponse));
        }
    }

    pub(crate) fn failed(&self) -> bool {
        self.attempted_at.is_some()
            && !self.connected
            && (self.provider.is_some()
                || self
                    .probes
                    .iter()
                    .any(|probe| probe.path == "configuration"))
    }

    pub(crate) fn status(&self) -> &'static str {
        if self.recorder_only {
            "Usage recorder only"
        } else if self
            .probes
            .iter()
            .any(|probe| probe.path == "configuration" && probe.issue.is_some())
        {
            "Configuration invalid"
        } else if self.attempted_at.is_none() {
            "Awaiting sample"
        } else if self.connected && self.probes.iter().any(|probe| probe.issue.is_some()) {
            "Connected · partial availability"
        } else if self.connected {
            "Connected"
        } else if self.provider.is_none() {
            "No runtime detected"
        } else {
            "Connection unavailable"
        }
    }

    pub(crate) fn usage_file(&mut self, path: Option<&Path>, has_records: bool) {
        self.usage = match path {
            None => "not configured",
            Some(_) if has_records => "matching completed usage available",
            Some(path) if !path.exists() => "configured file does not exist",
            Some(path) if !path.is_file() || std::fs::File::open(path).is_err() => {
                "configured path is not a readable file"
            }
            Some(_) => "no valid matching usage records",
        };
    }

    pub(crate) fn lines(&self, sample: &Sample) -> Vec<String> {
        let mut lines = vec![
            format!(
                "Runtime: {} · {}",
                self.provider.as_deref().unwrap_or("none"),
                self.status()
            ),
            format!(
                "Selection: {}",
                if self.selection.is_empty() {
                    "automatic"
                } else {
                    self.selection
                }
            ),
            format!(
                "Endpoint: {} · {}",
                if self.endpoint.is_empty() {
                    "—"
                } else {
                    &self.endpoint
                },
                if self.remote {
                    "remote API; host counters are local"
                } else {
                    "local"
                }
            ),
            format!(
                "Credentials: {}",
                if self.credentials_present {
                    "configured (hidden)"
                } else {
                    "not configured"
                }
            ),
            format!(
                "Last attempt: {} · last API success: {}",
                telemetry_age(self.attempted_at),
                telemetry_age(self.succeeded_at)
            ),
            format!(
                "Measurements: {} · {}",
                sample.llm_source.label(),
                telemetry_age(sample.llm_observed_at)
            ),
        ];
        if let Some(details) = &sample.llm_details {
            lines.push(format!(
                "Runtime details: {}",
                details
                    .chars()
                    .filter(|c| !c.is_control())
                    .collect::<String>()
            ));
        }
        for probe in &self.probes {
            lines.push(format!(
                "{}: {}",
                probe.path,
                probe
                    .issue
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "responded".into())
            ));
        }
        lines.push(format!(
            "Usage recorder: {}",
            if self.usage.is_empty() {
                "not configured"
            } else {
                self.usage
            }
        ));
        lines.push(String::new());
        lines.push("CAPABILITIES · source / observed availability".into());
        let provider = self.provider.as_deref().unwrap_or("");
        for capability in capabilities(provider) {
            let observed = capability.observed(sample);
            lines.push(format!(
                "{}: {} · {}",
                capability.name, capability.source, observed
            ));
        }
        lines.push(String::new());
        lines.push("SETUP".into());
        lines.extend(setup(provider).iter().map(|line| (*line).into()));
        if self.remote {
            lines.push(
                "Run mlxtop over SSH on the serving host to correlate inference with its hardware."
                    .into(),
            );
        }
        lines
    }
}

struct Capability {
    name: &'static str,
    source: &'static str,
}
impl Capability {
    fn observed(&self, sample: &Sample) -> String {
        let requests = || {
            sample
                .llm_requests
                .iter()
                .filter(|request| match self.name {
                    "Generation" => request.output_tps.is_some(),
                    "Cache" => request.cached.is_some(),
                    "Request first token" => request.ttft_ms.is_some(),
                    _ => true,
                })
                .collect::<Vec<_>>()
        };
        let present = match self.name {
            "Models" => {
                sample.llm_source == TelemetrySource::Live
                    && !matches!(sample.llm_model.as_str(), "not detected" | "unknown")
            }
            "Generation" => sample.llm_generation_tps.is_some() || !requests().is_empty(),
            "Prefill" => sample.llm_prefill_tps.is_some(),
            "Queue" => {
                sample.llm_active_requests.is_some() || sample.llm_waiting_requests.is_some()
            }
            "Cache" => {
                sample.llm_cache_efficiency.is_some()
                    || sample.llm_prefix_hit_rate.is_some()
                    || !requests().is_empty()
            }
            _ => !requests().is_empty(),
        };
        if self.source == "not provided" {
            return "unavailable".into();
        }
        if !present {
            return "not observed".into();
        }
        if matches!(self.name, "Requests" | "Request first token")
            || (self.name == "Generation" && sample.llm_generation_tps.is_none())
            || (self.name == "Cache"
                && sample.llm_cache_efficiency.is_none()
                && sample.llm_prefix_hit_rate.is_none())
        {
            let requests = requests();
            let at = requests.iter().filter_map(|r| r.observed_at).max();
            return format!(
                "{} · newest {}",
                if requests.iter().any(|r| !r.completed) {
                    "sampled request observations"
                } else {
                    "reported completions"
                },
                telemetry_age(at)
            );
        }
        let stale = sample.llm_status == "stale"
            || (sample.llm_source == TelemetrySource::Live
                && sample.llm_observed_at.is_some_and(|at| {
                    SystemTime::now()
                        .duration_since(at)
                        .is_ok_and(|age| age.as_secs_f64() > 5.0)
                }));
        if stale {
            return format!("stale · {}", telemetry_age(sample.llm_observed_at));
        }
        if matches!(self.name, "Generation" | "Prefill") {
            let live = if self.name == "Generation" {
                sample.llm_generation_tps_live
            } else {
                sample.llm_prefill_tps_live
            };
            return format!(
                "{} · {}",
                if sample.llm_source != TelemetrySource::Live {
                    "LAST result"
                } else if live {
                    "LIVE"
                } else {
                    "SERVER AVG"
                },
                telemetry_age(sample.llm_observed_at)
            );
        }
        if self.name == "Models" {
            return match sample.llm_status.as_str() {
                "available" => "available-model catalogue".into(),
                "loaded" => "loaded inventory".into(),
                "no models" => "empty inventory".into(),
                _ => "inventory observed".into(),
            };
        }
        format!("available · {}", telemetry_age(sample.llm_observed_at))
    }
}

fn capabilities(provider: &str) -> Vec<Capability> {
    let (models, generation, prefill, queue, requests, cache) = match provider {
        "oMLX" => (
            "native inventory",
            "native live / server average",
            "native live / server average",
            "native active / waiting",
            "sampled native requests",
            "native / request counters",
        ),
        "llama.cpp" => (
            "not provided",
            "native server average",
            "native server average",
            "native slots / metrics",
            "client recorder",
            "client recorder",
        ),
        "KoboldCpp" => (
            "not provided",
            "native last result",
            "native last result",
            "not provided",
            "native last result",
            "not provided",
        ),
        "Ollama" => (
            "native loaded models",
            "client recorder",
            "not provided",
            "not provided",
            "client recorder",
            "client recorder",
        ),
        "LM Studio" => (
            "native loaded models / catalogue fallback",
            "client recorder",
            "not provided",
            "not provided",
            "client recorder",
            "client recorder",
        ),
        "vLLM" | "SGLang" => (
            "metric labels when present",
            "sampled server rate; needs two counter samples",
            "sampled server rate; needs two counter samples",
            "native active / waiting",
            "client recorder",
            "native aggregate / client recorder",
        ),
        "mlx-serve" => (
            "native /metrics.json model",
            "native live rate of running requests",
            "native live prompt gauge (advances in chunks)",
            "native running / waiting",
            "native /metrics.json running requests",
            "native interval / cumulative",
        ),
        "mlx-lm" | "LocalAI" | "Jan" | "GPT4All" => (
            "native available-model catalogue",
            "client recorder",
            "not provided",
            "not provided",
            "client recorder",
            "client recorder",
        ),
        _ => return Vec::new(),
    };
    [
        ("Models", models),
        ("Generation", generation),
        ("Prefill", prefill),
        ("Queue", queue),
        ("Requests", requests),
        ("Cache", cache),
        ("Request first token", "explicit client timing required"),
    ]
    .into_iter()
    .map(|(name, source)| Capability { name, source })
    .collect()
}

fn setup(provider: &str) -> &'static [&'static str] {
    match provider {
        "oMLX" => &["Endpoint: ~/.config/mlxtop/config.json → omx.host / omx.port, or discovered oMLX server.env.", "Admin statistics require the existing oMLX API key configuration; health alone supplies partial monitoring.", "See docs/USER_GUIDE.md#omlx-telemetry for monitoring credentials."],
        "llama.cpp" => &["Enable the server's --metrics endpoint; /slots and /metrics can work independently.", "Select with MLXTOP_PROVIDER=llama.cpp; set MLXTOP_PROVIDER_PORT for a custom port.", "For full request history, integrate the client usage recorder and set MLXTOP_USAGE_FILE to its JSONL file."],
        "vLLM" | "SGLang" => &["Expose /metrics (SGLang requires --enable-metrics); rates need two matching counter samples.", "Set MLXTOP_PROVIDER=vllm or MLXTOP_PROVIDER=sglang and MLXTOP_PROVIDER_URL for a custom server root.", "Aggregate first-token timing is not individual request latency; request history requires client recording."],
        "mlx-serve" => &["Start mlx-serve with --metrics; mlxtop reads /metrics for rates and queues and /metrics.json for running requests.", "Select with MLXTOP_PROVIDER=mlx-serve (default port 11234); set MLXTOP_PROVIDER_PORT for a custom port.", "Bearer keys use MLXTOP_PROVIDER_API_KEY. Sessions that only hold a prefix cache are not listed as requests."],
        "KoboldCpp" => &["Enable the local server and check /api/extra/perf; it describes the last result, not a live request.", "Select with MLXTOP_PROVIDER=koboldcpp; set MLXTOP_PROVIDER_PORT for a custom port."],
        "Ollama" | "LM Studio" | "mlx-lm" | "LocalAI" | "Jan" | "GPT4All" => &["Enable the runtime's local API server and select it with MLXTOP_PROVIDER.", "Use MLXTOP_PROVIDER_PORT or MLXTOP_PROVIDER_URL for a custom endpoint; bearer keys use MLXTOP_PROVIDER_API_KEY.", "Integrate scripts/record_usage.py in the calling client so completed responses write counters to a JSONL file.", "Set MLXTOP_USAGE_FILE=/absolute/path/usage.jsonl when starting mlxtop. This variable alone does not record requests.", "Recorder setup: https://github.com/maximpri/mlxtop/blob/main/docs/USER_GUIDE.md#client-reported-usage-file"],
        _ => &["Start a supported runtime, or select an existing server with MLXTOP_PROVIDER.", "Providers: omlx, ollama, lmstudio, llama.cpp, koboldcpp, mlx-lm, mlx-serve, localai, vllm, sglang, jan, gpt4all.", "Example: MLXTOP_PROVIDER=ollama mlxtop doctor. Monitoring never sends inference requests."],
    }
}

/// Do not echo malformed URLs: they may contain a credential or terminal control.
pub(crate) fn safe_endpoint(base: &str) -> Option<String> {
    let uri: ureq::http::Uri = base.parse().ok()?;
    (matches!(uri.scheme_str(), Some("http" | "https"))
        && uri.authority().is_some_and(|a| !a.as_str().contains('@'))
        && uri.query().is_none()
        && !base.contains('#')
        && !base.chars().any(char::is_control))
    .then(|| base.to_owned())
}

pub(crate) fn default_port(provider: Option<&str>) -> u16 {
    match provider {
        Some("Ollama") => 11434,
        Some("LM Studio") => 1234,
        Some("KoboldCpp") => 5001,
        Some("vLLM") => 8000,
        Some("SGLang") => 30000,
        Some("mlx-serve") => 11234,
        Some("Jan") => 6767,
        Some("GPT4All") => 4891,
        _ => 8080,
    }
}

#[cfg(test)]
#[path = "tests/runtime_diagnostics.rs"]
mod tests;
