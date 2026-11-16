// SPDX-License-Identifier: MIT
//! Shared, credential-free connection observations and capability guidance.
use crate::domain::{Sample, TelemetrySource};
use crate::formatting::telemetry_age;
use std::fmt;
use std::net::SocketAddr;
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
    /// A key is set, but no runtime process was found or chosen: it is not
    /// sent to whatever answers on the port.
    NotDetected,
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
            Self::CredentialsMissing => f.write_str("monitoring credentials not configured; set the API key in Settings (4), MLXTOP_PROVIDER_API_KEY, or auth.api_key in oMLX's settings"),
            Self::NotDetected => f.write_str("no oMLX process found; key not sent (set MLXTOP_PROVIDER=omlx to send it)"),
            Self::Timeout => f.write_str("connection timed out; check the server address and reachability"),
            Self::Transport => f.write_str("connection failed; check that the server is running and reachable"),
            Self::Http(401 | 403) => f.write_str("authentication rejected; check the API key in Settings (4) or MLXTOP_PROVIDER_API_KEY, and its permissions"),
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

    /// Why a connection to `address` was not made. On a loopback address
    /// the operating system completes the handshake for a listening server,
    /// hung or not, so a connect that times out there found no server:
    /// Windows retries a refused connect for about two seconds before it
    /// says so. That is a refusal, retried within seconds. A hung server
    /// accepts and times out when read; a connect to it times out only once
    /// its queue of waiting connections is full, which a retry does not add to.
    pub(crate) fn from_connect(error: std::io::Error, address: &SocketAddr) -> Self {
        match Self::from_io(error) {
            Self::Timeout if address.ip().is_loopback() => Self::Transport,
            issue => issue,
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

/// What a runtime's answers say about the key it wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyNeed {
    /// It asks for a key and none was found to send.
    Missing,
    /// The key that was sent was refused.
    Rejected,
}

impl RuntimeReport {
    /// Whether the runtime needs a key it was not given, or refused the one
    /// it got. `None` while it answers, is down, or has not been asked.
    pub(crate) fn key_need(&self) -> Option<KeyNeed> {
        let issue = |wanted: fn(&ProbeIssue) -> bool| {
            self.probes
                .iter()
                .any(|probe| probe.issue.as_ref().is_some_and(wanted))
        };
        if issue(|issue| *issue == ProbeIssue::CredentialsMissing) {
            Some(KeyNeed::Missing)
        } else if issue(|issue| matches!(issue, ProbeIssue::Http(401 | 403))) {
            Some(if self.credentials_present {
                KeyNeed::Rejected
            } else {
                KeyNeed::Missing
            })
        } else {
            None
        }
    }

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
            if self.endpoint.is_empty() {
                // No runtime asked: no endpoint, local or remote.
                "Endpoint: —".into()
            } else {
                format!(
                    "Endpoint: {} · {}",
                    self.endpoint,
                    if self.remote {
                        "remote API; host counters are local"
                    } else {
                        "local"
                    }
                )
            },
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
        lines.extend(sample.native_runtime_lines());
        lines.extend(crate::model_facts::lines(&sample.model));
        lines.extend(crate::omlx::draft_lines(sample));
        lines.extend(crate::providers::llama::lines(sample));
        // How much more context fits, worked out from those facts.
        lines.extend(crate::context_left::lines(sample));
        // What requests lost before their first tokens, and the evidence.
        lines.extend(crate::request_findings::lines(&sample.request_findings));
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
        let stale = sample.llm_source == TelemetrySource::Live && !sample.llm_fresh();
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
            "native /v1/models",
            "native live slots / server average",
            "native server average",
            "native slots / metrics",
            "native answering slots / client recorder",
            "native metrics and slots / client recorder",
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
        "TensorFold" => (
            "native available-model catalogue",
            "live decode tokens; completion-counter averages otherwise",
            "sampled completed-request throughput; needs two counter samples",
            "native active / waiting",
            "client recorder",
            "native aggregate / client recorder",
        ),
        "Strata" => (
            "native available-model catalogue",
            "live engine gauge; completion-counter averages otherwise",
            "live engine gauge; completion-counter averages otherwise",
            "native active / waiting",
            "client recorder",
            "native aggregate / client recorder",
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
        "Splash" => (
            "native /status model",
            "sampled engine decode counter",
            "native prefill tokens over prefill GPU time",
            "native running / waiting",
            "native /status running requests",
            "native prefix-cache tokens, interval / cumulative",
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
        "oMLX" => &["Endpoint: ~/.config/mlxtop/config.json → omx.host / omx.port, or discovered from server.env, OMLX_PORT or ~/.omlx/settings.json (default port 8000).", "Admin statistics need oMLX's API key: read from MLXTOP_PROVIDER_API_KEY, the API key in Settings (4), OMLX_API_KEY, ~/.config/omlx-coding/server.env (API_KEY=) or auth.api_key in ~/.omlx/settings.json. Health alone gives partial monitoring.", "Credentials: https://github.com/maximpri/mlxtop/blob/main/docs/USER_GUIDE.md#omlx-telemetry"],
        "llama.cpp" => &["Enable the server's --metrics endpoint: mlxtop then reads /slots only while a request runs, so it never wakes a sleeping server or keeps an idle one awake. Without it, each /slots read counts as activity.", "Select with MLXTOP_PROVIDER=llama.cpp; mlxtop tries port 9931, then 8080. Set MLXTOP_PROVIDER_PORT for a custom port.", "For full request history, integrate the client usage recorder and set MLXTOP_USAGE_FILE to its JSONL file."],
        "vLLM" | "SGLang" => &["Expose /metrics (SGLang requires --enable-metrics); rates need two matching counter samples.", "Set MLXTOP_PROVIDER=vllm or MLXTOP_PROVIDER=sglang and MLXTOP_PROVIDER_URL for a custom server root.", "Aggregate first-token timing is not individual request latency; request history requires client recording."],
        "mlx-serve" => &["Start mlx-serve with --metrics; mlxtop reads /metrics for rates and queues and /metrics.json for running requests. Sushi serves the same endpoints under `sushi:` gauge names and needs no flag.", "Select with MLXTOP_PROVIDER=mlx-serve (default port 11234); set MLXTOP_PROVIDER_PORT for a custom port.", "Bearer keys use MLXTOP_PROVIDER_API_KEY. Sessions that only hold a prefix cache are not listed as requests."],
        "Splash" => &["mlxtop reads Splash's /status: running requests, decode and prefill counters, Metal memory and prefix-cache reuse.", "Select with MLXTOP_PROVIDER=splash (default port 8000); set MLXTOP_PROVIDER_PORT to match splash serve --port.", "With splash serve --api-key or SPLASH_API_KEY, set the same key in MLXTOP_PROVIDER_API_KEY."],
        "TensorFold" => &["mlxtop reads /metrics and /v1/models; no dashboard flag is needed.", "Select with MLXTOP_PROVIDER=tensorfold (default port 8080); bearer keys use MLXTOP_PROVIDER_API_KEY.", "Token counters advance at request completion, not during decode. Individual request history requires the client usage recorder."],
        "Strata" => &["mlxtop reads /metrics?format=prometheus (vLLM's gauge names plus Strata's own live rates) and /v1/models; no dashboard flag is needed.", "Select with MLXTOP_PROVIDER=strata (default port 8080 through the launcher, 8095 for a bare strata serve); set MLXTOP_PROVIDER_PORT or MLXTOP_PROVIDER_URL for a custom server root.", "Bearer keys use MLXTOP_PROVIDER_API_KEY. Individual request history requires the client usage recorder."],
        "KoboldCpp" => &["Enable the local server and check /api/extra/perf; it describes the last result, not a live request.", "Select with MLXTOP_PROVIDER=koboldcpp; set MLXTOP_PROVIDER_PORT for a custom port."],
        "Ollama" | "LM Studio" | "mlx-lm" | "LocalAI" | "Jan" | "GPT4All" => &["Enable the runtime's local API server and select it with MLXTOP_PROVIDER.", "Use MLXTOP_PROVIDER_PORT or MLXTOP_PROVIDER_URL for a custom endpoint; bearer keys use MLXTOP_PROVIDER_API_KEY.", "Integrate scripts/record_usage.py in the calling client so completed responses write counters to a JSONL file.", "Set MLXTOP_USAGE_FILE=/absolute/path/usage.jsonl when starting mlxtop. This variable alone does not record requests.", "Recorder setup: https://github.com/maximpri/mlxtop/blob/main/docs/USER_GUIDE.md#client-reported-usage-file"],
        _ => &["Start a supported runtime, or select an existing server with MLXTOP_PROVIDER.", "Providers: omlx, ollama, lmstudio, llama.cpp, koboldcpp, mlx-lm, mlx-serve, splash, localai, vllm, sglang, jan, gpt4all, tensorfold, strata.", "Example: MLXTOP_PROVIDER=ollama mlxtop doctor. Monitoring never sends inference requests."],
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

/// The ports a runtime listens on by default, most likely first.
pub(crate) fn default_ports(provider: Option<&str>) -> &'static [u16] {
    match provider {
        Some("llama.cpp") => &[9931, 8080],
        Some("Jan") => &[1337, 6767],
        Some("Strata") => &[8080, 8095],
        _ => &[],
    }
}

pub(crate) fn default_port(provider: Option<&str>) -> u16 {
    if let [port, ..] = default_ports(provider) {
        return *port;
    }
    match provider {
        Some("Ollama") => 11434,
        Some("LM Studio") => 1234,
        Some("KoboldCpp") => 5001,
        Some("vLLM" | "Splash") => 8000,
        Some("SGLang") => 30000,
        Some("mlx-serve") => 11234,
        Some("GPT4All") => 4891,
        _ => 8080,
    }
}

#[cfg(test)]
#[path = "tests/runtime_diagnostics.rs"]
mod tests;
