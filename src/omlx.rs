// SPDX-License-Identifier: MIT
//! oMLX monitoring, login, endpoint discovery and telemetry normalization.
use crate::config::{Config, DEFAULT_OMLX_HOST, DEFAULT_OMLX_PORT};
use crate::domain::{LlmTelemetry, MlxTelemetry, RequestUsage, Sample, SsdCache, TelemetrySource};
use crate::host::Host;
use crate::json::{
    gib_to_bytes, json_f64, json_f64_key, json_string, json_string_paths_or_keys, json_u64,
    json_u64_paths_or_keys,
};
use crate::logging::{diagnostics_log, log_field};
use crate::model_facts::{mlx_facts, Engine, KvFormat, MlxModel, ModelFacts, ModelFiles};
use crate::providers;
use crate::runtime_diagnostics::{ProbeIssue, RuntimeReport};
use crate::transport::{http_request, HttpResponse};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{IpAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use std::{env, fs};
pub(crate) struct LlmTelemetryClient {
    pub(crate) provider_adapter: providers::Adapter,
    pub(crate) home: Option<PathBuf>,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) session_cookie: Option<String>,
    pub(crate) cached: Option<LlmTelemetry>,
    pub(crate) mlx_metadata: MlxTelemetry,
    pub(crate) last_stats_available: Option<bool>,
    pub(crate) next_metadata_poll: Instant,
    pub(crate) next_poll: Instant,
    pub(crate) retry_backoff: Duration,
    pub(crate) runtime: RuntimeReport,
    using_adapter: bool,
    configured_endpoint: bool,
    /// Whether an oMLX was detected or chosen: only then is its port asked
    /// anything, the admin key included, not whatever answers there.
    pub(crate) login_allowed: bool,
    /// When a login was last refused, and why: a wrong key is retried once
    /// a minute, not on every poll.
    login_refused_at: Option<(Instant, ProbeIssue)>,
    /// Whether oMLX serves its admin statistics without a login, as it does
    /// with "Disable all authentication" on a loopback bind: `None` until
    /// asked, and asked again once a login finds no key configured there or
    /// a minute after it last refused.
    pub(crate) open_admin: Option<(bool, Instant)>,
    /// The key set in mlxtop's own config, when there is one.
    api_key: Option<String>,
    decode: crate::decode_tracker::DecodeTracker,
    /// Reads the served model's files for its facts.
    pub(crate) files: Box<dyn Host>,
    /// What oMLX says about how it serves its models.
    pub(crate) models: OmlxModels,
}

impl LlmTelemetryClient {
    pub(crate) fn from_config(config: &Config, home: Option<PathBuf>) -> Self {
        let (host, port) = read_omlx_endpoint(config, home.as_deref());
        Self {
            provider_adapter: providers::Adapter::new(config.api_key()),
            api_key: config.api_key(),
            home,
            host,
            port,
            session_cookie: None,
            cached: None,
            mlx_metadata: MlxTelemetry::default(),
            last_stats_available: None,
            next_metadata_poll: Instant::now(),
            next_poll: Instant::now(),
            retry_backoff: Duration::from_secs(1),
            runtime: RuntimeReport::default(),
            using_adapter: false,
            login_allowed: config
                .omx
                .as_ref()
                .is_some_and(|omx| omx.host.is_some() || omx.port.is_some()),
            login_refused_at: None,
            open_admin: None,
            decode: Default::default(),
            files: Box::new(crate::host::System),
            models: OmlxModels::default(),
            configured_endpoint: config
                .omx
                .as_ref()
                .is_some_and(|omx| omx.host.is_some() || omx.port.is_some()),
        }
    }

    pub(crate) fn poll(&mut self, detected_provider: Option<&str>) -> Option<LlmTelemetry> {
        self.using_adapter = self.provider_adapter.selected(detected_provider);
        if self.using_adapter {
            return providers::with_poll_deadline(|| self.provider_adapter.poll());
        }
        self.login_allowed = detected_provider == Some("oMLX")
            || self.configured_endpoint
            || self.provider_adapter.provider() == Some("oMLX");
        if !self.login_allowed {
            return self.no_runtime();
        }
        let now = Instant::now();
        if now < self.next_poll {
            return self.cached.clone();
        }
        self.runtime.provider = Some("oMLX".into());
        self.runtime.selection = if self.provider_adapter.explicitly_selected() {
            "MLXTOP_PROVIDER; oMLX endpoint config"
        } else if self.configured_endpoint {
            "oMLX endpoint config"
        } else {
            "process detection; oMLX endpoint config"
        };
        let authority = if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        self.runtime.endpoint =
            crate::runtime_diagnostics::safe_endpoint(&format!("http://{authority}:{}", self.port))
                .unwrap_or_else(|| "<invalid endpoint>".into());
        self.runtime.remote = !is_local_host(&self.host);
        self.runtime.credentials_present =
            omlx_api_key(self.home.as_deref(), self.api_key.as_deref()).is_some();
        self.runtime.begin();
        let telemetry = if self.runtime.endpoint == "<invalid endpoint>" {
            self.runtime
                .record("configuration", Err(ProbeIssue::InvalidEndpoint))
        } else {
            providers::with_poll_deadline(|| self.poll_once())
        };
        self.runtime.finish(telemetry.is_some());

        if let Some(mut telemetry) = telemetry {
            telemetry.remote = !is_local_host(&self.host);
            self.cached = Some(telemetry);
            self.retry_backoff = Duration::from_secs(1);
            self.next_poll = Instant::now() + self.retry_backoff;
        } else {
            diagnostics_log(
                "WARN",
                "llm_api_poll_failed",
                format!(
                    "host={} port={} retry_seconds={}",
                    log_field(&self.runtime.endpoint),
                    self.port,
                    self.retry_backoff.as_secs()
                ),
            );
            // Counted from the end of the poll; a refused connection is
            // retried within seconds, a timed-out one backs off further.
            let timed_out = self
                .runtime
                .probes
                .iter()
                .any(|probe| probe.issue == Some(ProbeIssue::Timeout));
            self.next_poll = Instant::now() + self.retry_backoff;
            self.retry_backoff = providers::next_backoff(self.retry_backoff, timed_out);
        }
        self.cached.clone()
    }

    /// No oMLX process was found, and oMLX was neither chosen nor configured:
    /// its port is asked nothing. Another runtime may listen there, unseen in
    /// a container or behind a tunnel, and SGLang answers `/health` by
    /// generating a token. An oMLX found later is asked at once.
    fn no_runtime(&mut self) -> Option<LlmTelemetry> {
        self.runtime = RuntimeReport {
            selection: "process detection",
            credentials_present: omlx_api_key(self.home.as_deref(), self.api_key.as_deref())
                .is_some(),
            attempted_at: Some(SystemTime::now()),
            succeeded_at: self.runtime.succeeded_at,
            ..RuntimeReport::default()
        };
        self.next_poll = Instant::now();
        self.retry_backoff = Duration::from_secs(1);
        self.cached.clone()
    }

    pub(crate) fn report(&self) -> RuntimeReport {
        if self.using_adapter {
            self.provider_adapter.report()
        } else {
            self.runtime.clone()
        }
    }

    fn request(
        &mut self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> Option<HttpResponse> {
        let response = self.runtime.record(
            path,
            http_request(&self.host, self.port, method, path, headers, body),
        )?;
        if response.status != 200 {
            self.runtime
                .record::<()>(path, Err(ProbeIssue::Http(response.status)));
        }
        Some(response)
    }

    pub(crate) fn poll_once(&mut self) -> Option<LlmTelemetry> {
        let health_response = match self.request("GET", "/health", &[], None) {
            Some(response) => response,
            None => {
                self.last_stats_available = None;
                diagnostics_log(
                    "WARN",
                    "llm_health_unreachable",
                    format!(
                        "host={} port={}",
                        log_field(&self.runtime.endpoint),
                        self.port
                    ),
                );
                return None;
            }
        };
        if health_response.status != 200 {
            self.last_stats_available = None;
            diagnostics_log(
                "WARN",
                "llm_health_http_error",
                format!(
                    "host={} port={} status={}",
                    log_field(&self.runtime.endpoint),
                    self.port,
                    health_response.status
                ),
            );
            return None;
        }
        let health: Value = match serde_json::from_str(&health_response.body) {
            Ok(health) => health,
            Err(error) => {
                self.last_stats_available = None;
                self.runtime
                    .record::<()>("/health", Err(ProbeIssue::InvalidResponse));
                diagnostics_log(
                    "WARN",
                    "llm_health_invalid_json",
                    format!(
                        "host={} port={} error={}",
                        log_field(&self.runtime.endpoint),
                        self.port,
                        log_field(&error.to_string())
                    ),
                );
                return None;
            }
        };
        if health.get("default_model").is_none() && health.get("engine_pool").is_none() {
            self.last_stats_available = None;
            self.runtime
                .record::<()>("/health", Err(ProbeIssue::InvalidResponse));
            diagnostics_log(
                "WARN",
                "llm_health_unrecognized",
                format!(
                    "host={} port={}",
                    log_field(&self.runtime.endpoint),
                    self.port
                ),
            );
            return None;
        }

        let stats = self.fetch_stats();
        let stats_available = stats.is_some();
        if self.last_stats_available != Some(stats_available) {
            diagnostics_log(
                if stats_available { "INFO" } else { "WARN" },
                "llm_api_stats",
                format!(
                    "host={} port={} available={stats_available}",
                    log_field(&self.runtime.endpoint),
                    self.port
                ),
            );
            self.last_stats_available = Some(stats_available);
        }
        let now = Instant::now();
        let refresh = now >= self.next_metadata_poll;
        if refresh {
            let device_info = self.fetch_json("/admin/api/device-info");
            let settings = self
                .fetch_json("/admin/api/global-settings")
                .or_else(|| self.fetch_json("/admin/api/settings"));
            if let Some(settings) = &settings {
                self.models.settings = OmlxSettings::parse(settings);
            }
            let metadata = parse_mlx_metadata(device_info.as_ref(), settings.as_ref());
            let metadata_available = !mlx_metadata_is_empty(&metadata);
            self.mlx_metadata = merge_mlx_telemetry(&self.mlx_metadata, &metadata);
            self.next_metadata_poll = now
                + if metadata_available {
                    Duration::from_secs(60)
                } else {
                    Duration::from_secs(10)
                };
        }
        let mut telemetry = parse_omlx_telemetry(&health, stats.as_ref());
        // The speed since the last poll, not oMLX's average since the request
        // began, which hides a stall.
        if let Some(rate) = self.decode.apply(&mut telemetry.requests, Instant::now()) {
            telemetry.generation_tps = Some(rate);
            telemetry.generation_tps_live = true;
        }
        telemetry.mlx = merge_mlx_telemetry(
            &self.mlx_metadata,
            &parse_mlx_runtime_telemetry(&health, stats.as_ref()),
        );
        telemetry.model_facts =
            self.model_facts(telemetry.model.as_deref(), stats.as_ref(), refresh);
        telemetry.max_concurrent = self.models.settings.max_concurrent_requests;
        telemetry.details = telemetry.model.as_deref().and_then(|id| {
            let splash = self
                .models
                .status
                .iter()
                .any(|status| status.id == id && status.engine.as_deref() == Some("splash"));
            self.models.server.slowing(id, splash)
        });
        telemetry.observed_at = Some(SystemTime::now());
        Some(telemetry)
    }

    /// The facts of the one model oMLX serves now, from `/v1/models/status`,
    /// oMLX's settings and the model's own files. Each is asked for again
    /// only on the metadata poll (`refresh`), or once when another model
    /// comes in; files are read again only when they changed.
    fn model_facts(
        &mut self,
        model: Option<&str>,
        stats: Option<&Value>,
        refresh: bool,
    ) -> ModelFacts {
        // Facts belong to a loaded model; several models at once have none.
        let Some((id, loaded)) = model.zip(stats).and_then(|(id, stats)| {
            let loaded = stats
                .pointer("/active_models/models")?
                .as_array()?
                .iter()
                .find(|entry| json_string(entry, &["id"]).as_deref() == Some(id))?;
            Some((id, loaded))
        }) else {
            return ModelFacts::default();
        };
        let listed = self.models.status.iter().any(|status| status.id == id);
        if refresh || (!listed && self.models.asked.as_deref() != Some(id)) {
            self.models.asked = Some(id.to_owned());
            if let Some(status) = self.fetch_api_json("/v1/models/status") {
                self.models.status = parse_models_status(&status);
            }
            if let Some(status) = self.fetch_api_json("/api/status") {
                // A server that stops naming its version keeps the one it gave.
                let server = ServerStatus::parse(&status);
                self.models.server = ServerStatus {
                    version: server.version.or(self.models.server.version.take()),
                    ..server
                };
            }
        }
        // A remote oMLX names files on its own machine, not this one.
        let local = is_local_host(&self.host);
        let base = self
            .models
            .settings
            .base_path
            .clone()
            .or_else(|| json_string(stats?, &["runtime_cache", "base_path"]).map(PathBuf::from))
            .filter(|_| local);
        let model_settings = base.and_then(|base| {
            self.models
                .model_settings(
                    self.files.as_ref(),
                    &base.join("model_settings.json"),
                    refresh,
                )
                .map(|all| all.get(id).cloned().unwrap_or_default())
        });
        let status = self
            .models
            .status
            .iter()
            .find(|status| status.id == id)
            .cloned();
        let files = self.files.as_ref();
        let mut read = |dir: Option<&Path>| {
            dir.filter(|_| local)
                .and_then(|dir| self.models.files.model(files, dir, refresh))
        };
        let model = read(status.as_ref().and_then(|status| status.path.as_deref()));
        // The configured draft is read only where a draft runs.
        let drafting = through_splash(status.as_ref(), model_settings.as_ref())
            || model_settings
                .as_ref()
                .is_some_and(|settings| settings.dflash);
        let draft = read(
            model_settings
                .as_ref()
                .and_then(|settings| settings.draft.as_deref())
                .filter(|_| drafting),
        );
        omlx_model_facts(&Serving {
            status: status.as_ref(),
            settings: &self.models.settings,
            model_settings: model_settings.as_ref(),
            model: model.as_ref(),
            draft: draft.as_ref(),
            version: self.models.server.version.as_deref(),
            loaded: Some(loaded),
        })
    }

    /// JSON from an endpoint that takes oMLX's API key as a bearer token,
    /// sent under the admin login's rules: to an oMLX found or chosen, on
    /// a loopback address unless remote authentication is allowed.
    fn fetch_api_json(&mut self, path: &str) -> Option<Value> {
        let may_send = self.login_allowed
            && (is_loopback_host(&self.host)
                || env::var("MLXTOP_ALLOW_REMOTE_AUTH").as_deref() == Ok("1"));
        let bearer = may_send
            .then(|| omlx_api_key(self.home.as_deref(), self.api_key.as_deref()))
            .flatten()
            .map(|key| format!("Bearer {key}"));
        let headers: Vec<(&str, &str)> = bearer
            .iter()
            .map(|value| ("Authorization", value.as_str()))
            .collect();
        let response = self.request("GET", path, &headers, None)?;
        if response.status != 200 {
            return None;
        }
        self.runtime.record(
            path,
            serde_json::from_str(&response.body).map_err(|_| ProbeIssue::InvalidResponse),
        )
    }

    pub(crate) fn fetch_stats(&mut self) -> Option<Value> {
        self.fetch_json("/admin/api/stats?scope=session")
    }

    /// Whether to ask for admin statistics without a login: oMLX answered
    /// one so, or it may, as nothing could log in.
    fn may_be_open(&self) -> bool {
        let known = self
            .open_admin
            .filter(|(open, at)| *open || at.elapsed() < Duration::from_secs(60));
        self.login_allowed
            && known.map_or_else(
                || {
                    omlx_api_key(self.home.as_deref(), self.api_key.as_deref()).is_none()
                        || matches!(self.login_refused_at, Some((_, ProbeIssue::NoServerKey)))
                },
                |(open, _)| open,
            )
    }

    /// Ask `path` without a credential, so nothing but the request leaves the
    /// machine. oMLX answers 401 unless it runs with "Disable all
    /// authentication" on a loopback bind. `Err` means a login is needed.
    fn fetch_open(&mut self, path: &str) -> Result<Option<Value>, ()> {
        let Some(response) = self.request("GET", path, &[], None) else {
            return Ok(None);
        };
        match response.status {
            200 => {
                self.open_admin = Some((true, Instant::now()));
                // No login is needed, so a refused one is no failure.
                self.runtime.forget("/admin/api/login");
                Ok(self.runtime.record(
                    path,
                    serde_json::from_str(&response.body).map_err(|_| ProbeIssue::InvalidResponse),
                ))
            }
            401 | 403 => {
                self.open_admin = Some((false, Instant::now()));
                // The login decides what this endpoint reports.
                self.runtime.forget(path);
                Err(())
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn fetch_json(&mut self, path: &str) -> Option<Value> {
        if self.session_cookie.is_none() && self.may_be_open() {
            if let Ok(value) = self.fetch_open(path) {
                return value;
            }
        }
        if self.session_cookie.is_none() {
            self.login();
            // The login found no key configured in oMLX: it may need none.
            if self.session_cookie.is_none() && self.open_admin.is_none() && self.may_be_open() {
                return self.fetch_open(path).ok().flatten();
            }
        }
        let cookie = self.session_cookie.clone()?;
        let response = self.request("GET", path, &[("Cookie", cookie.as_str())], None)?;
        if response.status == 401 {
            self.session_cookie = None;
            self.login();
            let cookie = self.session_cookie.clone()?;
            let response = self.request("GET", path, &[("Cookie", cookie.as_str())], None)?;
            if response.status != 200 {
                return None;
            }
            return self.runtime.record(
                path,
                serde_json::from_str(&response.body).map_err(|_| ProbeIssue::InvalidResponse),
            );
        }
        if response.status != 200 {
            return None;
        }
        self.runtime.record(
            path,
            serde_json::from_str(&response.body).map_err(|_| ProbeIssue::InvalidResponse),
        )
    }

    pub(crate) fn login(&mut self) {
        if !is_loopback_host(&self.host)
            && env::var("MLXTOP_ALLOW_REMOTE_AUTH").as_deref() != Ok("1")
        {
            self.runtime
                .record::<()>("/admin/api/login", Err(ProbeIssue::RemoteAuthBlocked));
            return;
        }
        let Some(api_key) = omlx_api_key(self.home.as_deref(), self.api_key.as_deref()) else {
            self.runtime
                .record::<()>("/admin/api/login", Err(ProbeIssue::CredentialsMissing));
            return;
        };
        if !self.login_allowed {
            // Something answers like oMLX on the polled port, but no oMLX
            // process was found and none was chosen: keep the key.
            self.runtime
                .record::<()>("/admin/api/login", Err(ProbeIssue::NotDetected));
            return;
        }
        if let Some((_, issue)) = self
            .login_refused_at
            .as_ref()
            .filter(|(at, _)| at.elapsed() < Duration::from_secs(60))
        {
            let issue = issue.clone();
            self.runtime.record::<()>("/admin/api/login", Err(issue));
            return;
        }
        let body = json!({ "api_key": api_key, "remember": true }).to_string();
        let Some(response) = self.request(
            "POST",
            "/admin/api/login",
            &[("Content-Type", "application/json")],
            Some(&body),
        ) else {
            return;
        };
        if matches!(response.status, 401 | 403 | 429) {
            // A wrong key, or oMLX's lockout after too many: wait a minute.
            let issue = ProbeIssue::Http(response.status);
            self.login_refused_at = Some((Instant::now(), issue.clone()));
            self.runtime.record::<()>("/admin/api/login", Err(issue));
            return;
        }
        if response.status == 400 && response.body.contains("No API key configured") {
            // oMLX has no key to log in with. Ask again in a minute, and ask
            // for the statistics without a login then: authentication may
            // have been turned off meanwhile.
            self.login_refused_at = Some((Instant::now(), ProbeIssue::NoServerKey));
            self.open_admin = None;
            self.runtime
                .record::<()>("/admin/api/login", Err(ProbeIssue::NoServerKey));
            return;
        }
        self.login_refused_at = None;
        if response.status == 200 {
            self.session_cookie = response
                .header("set-cookie")
                .and_then(|value| value.split(';').next())
                .map(str::to_owned);
            if self.session_cookie.is_none() {
                self.runtime
                    .record::<()>("/admin/api/login", Err(ProbeIssue::InvalidResponse));
            }
        }
    }
}

/**
 * Endpoint values discovered from `~/.config/omlx-coding/server.env` or
 * oMLX's own settings.
 *
 * `None` means the file said nothing usable about that field, which keeps
 * "absent" distinct from "explicitly set to the built-in default".
 */
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DiscoveredEndpoint {
    pub(crate) host: Option<String>,
    pub(crate) port: Option<u16>,
}

pub(crate) fn parse_omlx_server_env(text: &str) -> DiscoveredEndpoint {
    let mut discovered = DiscoveredEndpoint::default();
    for line in text.lines() {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches(['"', '\'']);
        match key.trim() {
            "HOST" | "OMLX_HOST" if !value.is_empty() && value != "0.0.0.0" => {
                discovered.host = Some(value.to_owned());
            }
            "PORT" | "OMLX_PORT" => {
                if let Ok(port) = value.parse::<u16>() {
                    discovered.port = Some(port);
                }
            }
            _ => {}
        }
    }
    discovered
}

/**
 * Resolve the endpoint to monitor.
 *
 * Discovery supplies the defaults; an explicitly configured host or port
 * always wins, because the user picked it deliberately. Each field is
 * resolved on its own, so configuring only a port keeps the discovered host.
 * A field that is neither configured nor discovered falls back to the
 * built-in default.
 */
pub(crate) fn resolve_omlx_endpoint(
    discovered: &DiscoveredEndpoint,
    config: &Config,
) -> (String, u16) {
    let configured = config.omx.as_ref();
    let host = configured
        .and_then(|omx| omx.host.clone())
        .filter(|host| !host.trim().is_empty())
        .or_else(|| discovered.host.clone())
        .unwrap_or_else(|| DEFAULT_OMLX_HOST.to_owned());
    let port = configured
        .and_then(|omx| omx.port)
        .or(discovered.port)
        .unwrap_or(DEFAULT_OMLX_PORT);
    (host, port)
}

/// The endpoint a stock oMLX keeps in `settings.json`: `server.host` and
/// `server.port`.
pub(crate) fn parse_omlx_settings(text: &str) -> DiscoveredEndpoint {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return DiscoveredEndpoint::default();
    };
    let server = value.get("server");
    DiscoveredEndpoint {
        host: server
            .and_then(|server| server.get("host"))
            .and_then(Value::as_str)
            .filter(|host| !host.is_empty() && *host != "0.0.0.0")
            .map(str::to_owned),
        port: server
            .and_then(|server| server.get("port"))
            .and_then(Value::as_u64)
            .and_then(|port| u16::try_from(port).ok())
            .filter(|port| *port > 0),
    }
}

/**
 * Where oMLX says it listens. A `server.env` written for oMLX wins; then
 * oMLX's own `OMLX_PORT`, then `settings.json` in `OMLX_BASE_PATH` (by
 * default `~/.omlx`). Each field is taken from the first source that has it.
 */
pub(crate) fn discover_omlx_endpoint(
    home: Option<&Path>,
    base_path: Option<&Path>,
    env_port: Option<&str>,
) -> DiscoveredEndpoint {
    let server_env = home
        .map(|home| home.join(".config/omlx-coding/server.env"))
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|text| parse_omlx_server_env(&text))
        .unwrap_or_default();
    let settings = base_path
        .map(Path::to_path_buf)
        .or_else(|| home.map(|home| home.join(".omlx")))
        .map(|base| base.join("settings.json"))
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|text| parse_omlx_settings(&text))
        .unwrap_or_default();
    let env_port = env_port
        .and_then(|port| port.trim().parse::<u16>().ok())
        .filter(|port| *port > 0);
    DiscoveredEndpoint {
        host: server_env.host.or(settings.host),
        port: server_env.port.or(env_port).or(settings.port),
    }
}

pub(crate) fn read_omlx_endpoint(config: &Config, home: Option<&Path>) -> (String, u16) {
    let base_path = std::env::var_os("OMLX_BASE_PATH").map(std::path::PathBuf::from);
    let env_port = std::env::var("OMLX_PORT").ok();
    let discovered = discover_omlx_endpoint(home, base_path.as_deref(), env_port.as_deref());
    let (host, port) = resolve_omlx_endpoint(&discovered, config);
    if discovered
        .host
        .as_deref()
        .is_some_and(|value| value != host)
        || discovered.port.is_some_and(|value| value != port)
    {
        diagnostics_log(
            "INFO",
            "omlx_endpoint_override",
            format!(
                "configured={host}:{port} discovered={}:{}",
                discovered.host.as_deref().unwrap_or("-"),
                discovered
                    .port
                    .map(|port| port.to_string())
                    .unwrap_or_else(|| "-".to_owned())
            ),
        );
    }
    (host, port)
}

pub(crate) fn is_loopback_host(host: &str) -> bool {
    matches!(
        host.trim_matches(['[', ']']),
        "127.0.0.1" | "localhost" | "::1"
    )
}

/// Whether `host` is this machine: a loopback host, or an address one of its
/// own interfaces holds, as a server bound to its LAN or Thunderbolt address.
pub(crate) fn is_local_host(host: &str) -> bool {
    is_loopback_host(host)
        || host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(held_here)
}

/// Binding a socket to an address fails unless one of this machine's
/// interfaces holds it. The wildcard and multicast addresses bind anywhere,
/// so they never count.
fn held_here(ip: IpAddr) -> bool {
    !ip.is_unspecified() && !ip.is_multicast() && UdpSocket::bind((ip, 0)).is_ok()
}

pub(crate) fn read_omlx_api_key(home: Option<&Path>) -> Option<String> {
    let path = home?.join(".config/omlx-coding/server.env");
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .find_map(|line| {
            let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
            let (key, value) = line.split_once('=')?;
            (key.trim() == "API_KEY").then(|| value.trim().trim_matches(['"', '\'']).to_owned())
        })
        .filter(|value| !value.is_empty())
}

/// The key a stock oMLX keeps as `auth.api_key` in its own `settings.json`.
pub(crate) fn read_omlx_settings_api_key(base: &Path) -> Option<String> {
    let text = fs::read_to_string(base.join("settings.json")).ok()?;
    serde_json::from_str::<Value>(&text)
        .ok()?
        .get("auth")?
        .get("api_key")?
        .as_str()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
}

/// The oMLX admin key, from the first source that has one:
/// `MLXTOP_PROVIDER_API_KEY`, the key set in mlxtop's Settings, oMLX's own
/// `OMLX_API_KEY`, the wrapper's `~/.config/omlx-coding/server.env`, then
/// oMLX's `settings.json` (in `OMLX_BASE_PATH`, by default `~/.omlx`).
pub(crate) fn omlx_api_key(home: Option<&Path>, configured: Option<&str>) -> Option<String> {
    // Tests read keys from files only, whatever the developer's shell holds.
    let var = |name: &str| {
        if cfg!(test) {
            None
        } else {
            env::var(name).ok()
        }
    };
    omlx_api_key_from(
        var("MLXTOP_PROVIDER_API_KEY"),
        configured.map(str::to_owned),
        var("OMLX_API_KEY"),
        home,
        env::var_os("OMLX_BASE_PATH").map(PathBuf::from).as_deref(),
    )
}

pub(crate) fn omlx_api_key_from(
    provider_key: Option<String>,
    configured: Option<String>,
    omlx_key: Option<String>,
    home: Option<&Path>,
    base_path: Option<&Path>,
) -> Option<String> {
    let set = |key: Option<String>| {
        key.map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())
    };
    set(provider_key)
        .or_else(|| set(configured))
        .or_else(|| set(omlx_key))
        .or_else(|| read_omlx_api_key(home))
        .or_else(|| {
            let base = base_path
                .map(Path::to_path_buf)
                .or_else(|| home.map(|home| home.join(".omlx")))?;
            read_omlx_settings_api_key(&base)
        })
}

/// Text from the runtime without control characters, at most 120 of them.
fn printable(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(120).collect()
}

pub(crate) fn parse_omlx_telemetry(health: &Value, stats: Option<&Value>) -> LlmTelemetry {
    let mut telemetry = LlmTelemetry {
        source: TelemetrySource::Live,
        provider: Some("oMLX".into()),
        status: Some(
            json_string(health, &["status"])
                .map(|status| printable(&status))
                .unwrap_or_else(|| "healthy".into()),
        ),
        model: json_string(health, &["default_model"]).map(|model| printable(&model)),
        model_memory: json_u64(health, &["engine_pool", "current_model_memory"]),
        model_memory_max: json_u64(health, &["engine_pool", "final_ceiling"]),
        mlx: parse_mlx_runtime_telemetry(health, stats),
        ..LlmTelemetry::default()
    };

    let Some(stats) = stats else {
        return telemetry;
    };
    telemetry.generation_tps =
        json_f64(stats, &["avg_generation_tps"]).filter(|value| *value >= 0.0);
    telemetry.prefill_tps = json_f64(stats, &["avg_prefill_tps"]).filter(|value| *value >= 0.0);
    telemetry.cache_efficiency =
        json_f64(stats, &["cache_efficiency"]).map(|value| value.clamp(0.0, 100.0));
    telemetry.requests = providers::omlx_requests(stats);
    telemetry.total_prompt_tokens = json_u64(stats, &["total_prompt_tokens"]);
    telemetry.total_cached_tokens = json_u64(stats, &["total_cached_tokens"]);
    telemetry.model_memory =
        json_u64(stats, &["active_models", "model_memory_used"]).or(telemetry.model_memory);
    telemetry.model_memory_max =
        json_u64(stats, &["active_models", "model_memory_max"]).or(telemetry.model_memory_max);

    // Headline values describe the whole server: loaded models share it, and
    // one model's queue or first request is not the server's.
    let models = stats
        .pointer("/active_models/models")
        .and_then(Value::as_array);
    let model_requests: Vec<_> = models
        .into_iter()
        .flatten()
        .map(|model| (model, providers::omlx_model_requests(model)))
        .collect();
    let busy: Vec<&Value> = model_requests
        .iter()
        .filter(|(model, requests)| {
            !requests.is_empty()
                || json_u64(model, &["active_requests"]).unwrap_or(0) > 0
                || json_u64(model, &["waiting_requests"]).unwrap_or(0) > 0
        })
        .map(|(model, _)| *model)
        .collect();
    let headline = busy
        .first()
        .copied()
        .or_else(|| models.and_then(|models| models.first()));
    let headline_id = headline.and_then(|model| json_string(model, &["id"]));
    if let Some(id) = &headline_id {
        // A distinct label also keeps multi-model throughput out of a single
        // model's correlation baseline.
        telemetry.model = Some(if busy.len() > 1 {
            format!("{} models · {id}", busy.len())
        } else {
            id.clone()
        });
    }
    let requests: Vec<&providers::OmlxRequest> = model_requests
        .iter()
        .flat_map(|(_, requests)| requests)
        .collect();
    let in_phase = |phase| -> Vec<&providers::OmlxRequest> {
        requests
            .iter()
            .copied()
            .filter(|request| request.phase == phase)
            .collect()
    };
    let generating = in_phase(providers::OmlxPhase::Generating);
    let prefilling = in_phase(providers::OmlxPhase::Prefilling);
    let waiting = in_phase(providers::OmlxPhase::Waiting);

    if let Some(models) = models {
        let total = |total: &str, per_model: &str| {
            json_u64(stats, &["active_models", total]).or_else(|| {
                models.iter().try_fold(0_u64, |sum, model| {
                    sum.checked_add(json_u64(model, &[per_model])?)
                })
            })
        };
        telemetry.active_requests = total("total_active_requests", "active_requests");
        telemetry.waiting_requests = total("total_waiting_requests", "waiting_requests");
    }
    // Concurrent requests add up; a partial sum is not presented as complete.
    let live_sum = |requests: &[&providers::OmlxRequest]| {
        (!requests.is_empty())
            .then(|| {
                requests
                    .iter()
                    .try_fold(0.0, |sum, request| Some(sum + request.rate?))
            })
            .flatten()
    };
    if let Some(rate) = live_sum(&generating) {
        telemetry.generation_tps = Some(rate);
        telemetry.generation_tps_live = true;
    }
    if let Some(rate) = live_sum(&prefilling) {
        telemetry.prefill_tps = Some(rate);
        telemetry.prefill_tps_live = true;
    }
    telemetry.output_tokens = (!generating.is_empty())
        .then(|| {
            generating
                .iter()
                .try_fold(0_u64, |sum, request| sum.checked_add(request.output?))
        })
        .flatten();
    // A prompt size describes one request; concurrent requests have none.
    let active = generating.len() + prefilling.len();
    telemetry.prompt_tokens = match (active, waiting.as_slice()) {
        (1, _) => generating
            .first()
            .or_else(|| prefilling.first())
            .and_then(|request| request.prompt),
        (0, [request]) => request.prompt.filter(|prompt| *prompt > 0),
        _ => None,
    };
    if !model_requests.is_empty() {
        let loading = model_requests.iter().any(|(model, _)| {
            model
                .get("is_loading")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        });
        // Serving work outranks another model's load.
        telemetry.status = Some(
            if !prefilling.is_empty() {
                "prefilling"
            } else if !generating.is_empty() {
                "generating"
            } else if loading {
                "loading"
            } else if telemetry.waiting_requests.unwrap_or(0) > 0 {
                "waiting"
            } else if telemetry.active_requests.unwrap_or(0) > 0 {
                "active"
            } else {
                "idle"
            }
            .into(),
        );
    }

    // Prefix reuse is per model, so several busy models have no single rate.
    // Older responses list one cache entry without a model ID.
    let model_caches = stats
        .pointer("/runtime_cache/models")
        .and_then(Value::as_array);
    let model_cache = model_caches.and_then(|caches| {
        if busy.len() > 1 {
            return None;
        }
        caches
            .iter()
            .find(|cache| headline_id.is_some() && json_string(cache, &["id"]) == headline_id)
            .or_else(|| match caches.as_slice() {
                [cache] if cache.get("id").is_none() => Some(cache),
                _ => None,
            })
    });
    telemetry.ssd_cache = parse_ssd_cache(stats);
    if let Some(model_cache) = model_cache {
        telemetry.prefix_hit_rate = json_f64(
            model_cache,
            &["cache_rates", "cumulative", "prefix_hit_rate"],
        )
        .map(|value| (value * 100.0).clamp(0.0, 100.0));
    }
    telemetry
}

/// oMLX's SSD prompt cache: its fill, and block loads, saves and evictions
/// summed over the models that use it. Absent when the cache is off.
pub(crate) fn parse_ssd_cache(stats: &Value) -> Option<SsdCache> {
    let cache = stats.get("runtime_cache")?;
    if cache
        .get("ssd_cache_unavailable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || cache.get("ssd_cache_dir").is_none_or(Value::is_null)
    {
        return None;
    }
    let models = cache
        .get("models")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let sum = |key: &str| -> u64 {
        models
            .iter()
            .filter_map(|model| json_u64(model, &["cache_rates", "cumulative", key]))
            .sum()
    };
    // The cache tier's own token counts, where it reports them (Splash).
    let tokens = |key: &str| -> Option<u64> {
        models
            .iter()
            .filter_map(|model| json_u64(model, &["splash_cache", key]))
            .reduce(u64::saturating_add)
    };
    Some(SsdCache {
        used: json_u64(cache, &["total_size_bytes"]).unwrap_or(0),
        capacity: json_u64(cache, &["disk_max_bytes"]).unwrap_or(0),
        loads: sum("ssd_disk_loads"),
        saves: sum("ssd_saves"),
        evictions: sum("evictions"),
        hit_tokens: tokens("kv_hit_tokens"),
        disk_hit_tokens: tokens("kv_disk_hit_tokens"),
    })
}

pub(crate) fn parse_mlx_runtime_telemetry(health: &Value, stats: Option<&Value>) -> MlxTelemetry {
    let mut sources = vec![health];
    if let Some(stats) = stats {
        sources.push(stats);
    }
    parse_mlx_sources(&sources)
}

pub(crate) fn parse_mlx_metadata(
    device_info: Option<&Value>,
    settings: Option<&Value>,
) -> MlxTelemetry {
    let mut sources = Vec::new();
    if let Some(device_info) = device_info {
        sources.push(device_info);
    }
    if let Some(settings) = settings {
        sources.push(settings);
    }
    parse_mlx_sources(&sources)
}

pub(crate) fn parse_mlx_sources(sources: &[&Value]) -> MlxTelemetry {
    let mut telemetry = MlxTelemetry::default();
    for source in sources {
        let active_memory = json_u64_paths_or_keys(
            source,
            &[
                &["mlx_memory", "active_bytes"],
                &["mlx", "active_bytes"],
                &["mlx", "active_memory"],
                &["active_memory_bytes"],
            ],
            &["mlx_active_memory_bytes"],
        );
        let cache_memory = json_u64_paths_or_keys(
            source,
            &[
                &["mlx_memory", "cache_bytes"],
                &["mlx", "cache_bytes"],
                &["mlx", "cache_memory"],
                &["cache_memory_bytes"],
            ],
            &["mlx_cache_memory_bytes"],
        );
        let peak_memory = json_u64_paths_or_keys(
            source,
            &[
                &["mlx_memory", "peak_bytes"],
                &["mlx", "peak_bytes"],
                &["mlx", "peak_memory"],
                &["peak_memory_bytes"],
            ],
            &["mlx_peak_memory_bytes"],
        );
        let process_footprint = json_u64_paths_or_keys(
            source,
            &[
                &["system", "omlx_phys_footprint_bytes"],
                &["system", "phys_footprint_bytes"],
            ],
            &["omlx_phys_footprint_bytes", "phys_footprint_bytes"],
        );
        let resource_limit = json_u64_paths_or_keys(
            source,
            &[
                &["system", "iogpu_wired_limit_bytes"],
                &["system", "metal_limit_bytes"],
                &["mlx", "resource_limit"],
            ],
            &[
                "iogpu_wired_limit_bytes",
                "metal_limit_bytes",
                "resource_limit",
            ],
        );
        let next = MlxTelemetry {
            version: json_string_paths_or_keys(
                source,
                &[
                    &["mlx_version"],
                    &["mlx", "version"],
                    &["engines", "mlx-lm", "version"],
                    &["engines", "mlx-vlm", "version"],
                    &["engines", "mlx-embeddings", "version"],
                    &["engines", "mlx-audio", "version"],
                ],
                &["mlx_version"],
            ),
            active_memory,
            cache_memory,
            peak_memory,
            device_name: json_string_paths_or_keys(
                source,
                &[
                    &["device_name"],
                    &["mlx_device_name"],
                    &["hardware", "device_name"],
                    &["chip_name"],
                ],
                &["device_name", "mlx_device_name", "chip_name"],
            ),
            architecture: json_string_paths_or_keys(
                source,
                &[&["architecture"], &["mlx", "architecture"]],
                &["architecture"],
            ),
            memory_size: json_u64_paths_or_keys(
                source,
                &[
                    &["memory_size"],
                    &["mlx", "memory_size"],
                    &["hardware", "memory_size"],
                    &["system", "total_memory_bytes"],
                ],
                &["memory_size", "total_memory_bytes"],
            )
            .or_else(|| json_f64_key(source, "memory_gb").and_then(gib_to_bytes)),
            recommended_working_set: json_u64_paths_or_keys(
                source,
                &[
                    &["max_recommended_working_set_size"],
                    &["mlx", "max_recommended_working_set_size"],
                    &["recommended_working_set_bytes"],
                ],
                &[
                    "max_recommended_working_set_size",
                    "recommended_working_set_bytes",
                ],
            ),
            max_buffer_size: json_u64_paths_or_keys(
                source,
                &[
                    &["max_buffer_size"],
                    &["mlx", "max_buffer_size"],
                    &["max_buffer_length"],
                ],
                &["max_buffer_size", "max_buffer_length"],
            ),
            resource_limit,
            process_footprint,
        };
        telemetry = merge_mlx_telemetry(&telemetry, &next);
    }
    telemetry
}

pub(crate) fn mlx_metadata_is_empty(telemetry: &MlxTelemetry) -> bool {
    telemetry.version.is_none()
        && telemetry.active_memory.is_none()
        && telemetry.cache_memory.is_none()
        && telemetry.peak_memory.is_none()
        && telemetry.device_name.is_none()
        && telemetry.architecture.is_none()
        && telemetry.memory_size.is_none()
        && telemetry.recommended_working_set.is_none()
        && telemetry.max_buffer_size.is_none()
        && telemetry.resource_limit.is_none()
        && telemetry.process_footprint.is_none()
}

pub(crate) fn merge_mlx_telemetry(base: &MlxTelemetry, update: &MlxTelemetry) -> MlxTelemetry {
    MlxTelemetry {
        version: update.version.clone().or_else(|| base.version.clone()),
        active_memory: update.active_memory.or(base.active_memory),
        cache_memory: update.cache_memory.or(base.cache_memory),
        peak_memory: update.peak_memory.or(base.peak_memory),
        device_name: update
            .device_name
            .clone()
            .or_else(|| base.device_name.clone()),
        architecture: update
            .architecture
            .clone()
            .or_else(|| base.architecture.clone()),
        memory_size: update.memory_size.or(base.memory_size),
        recommended_working_set: update
            .recommended_working_set
            .or(base.recommended_working_set),
        max_buffer_size: update.max_buffer_size.or(base.max_buffer_size),
        resource_limit: update.resource_limit.or(base.resource_limit),
        process_footprint: update.process_footprint.or(base.process_footprint),
    }
}

/// What oMLX says about how it serves its models, kept between polls: the
/// settings and status from the metadata poll, and the files read once.
#[derive(Debug, Default)]
pub(crate) struct OmlxModels {
    pub(crate) settings: OmlxSettings,
    /// `/v1/models/status`.
    pub(crate) status: Vec<ModelStatus>,
    /// `/api/status`.
    pub(crate) server: ServerStatus,
    model_settings: Option<SettingsFile>,
    files: ModelFiles,
    /// The model the status was last asked for between metadata polls.
    asked: Option<String>,
}

/// What oMLX's `/api/status`, which takes the API key alone, adds to its admin statistics: its
/// version, and what slows its own engine. A native kernel it could not load leaves the models
/// that use it on much slower generic code; a model whose Neural Engine prefill was shed reads
/// its prompts on the GPU alone. The import errors themselves are not kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ServerStatus {
    pub(crate) version: Option<String>,
    pub(crate) missing_kernels: Vec<String>,
    pub(crate) shed_prefill: Vec<String>,
}

impl ServerStatus {
    pub(crate) fn parse(status: &Value) -> Self {
        let kernels = status.get("custom_kernels").and_then(Value::as_object);
        let models = status
            .pointer("/ane_prefill/models")
            .and_then(Value::as_array);
        Self {
            version: json_string(status, &["version"]).map(|version| printable(&version)),
            missing_kernels: kernels
                .into_iter()
                .flatten()
                .filter(|(_, kernel)| {
                    kernel.get("available").and_then(Value::as_bool) == Some(false)
                })
                .map(|(name, _)| printable(name))
                .collect(),
            shed_prefill: models
                .into_iter()
                .flatten()
                .filter(|model| model.get("shed").and_then(Value::as_bool) == Some(true))
                .filter_map(|model| json_string(model, &["model_id"]))
                .map(|id| printable(&id))
                .collect(),
        }
    }

    /// What slows the model oMLX serves, in words; `splash` when Splash runs it, on kernels
    /// of its own.
    pub(crate) fn slowing(&self, model: &str, splash: bool) -> Option<String> {
        let mut said = Vec::new();
        if !splash && !self.missing_kernels.is_empty() {
            said.push(format!(
                "custom kernels not loaded: {}",
                self.missing_kernels.join(", ")
            ));
        }
        if self.shed_prefill.iter().any(|shed| shed == model) {
            said.push("Neural Engine prefill shed: prompts read on the GPU alone".into());
        }
        (!said.is_empty()).then(|| said.join(" · "))
    }
}

/// `model_settings.json` as last read: when it changed, and each model's
/// settings (`None` when it could not be read).
#[derive(Debug)]
struct SettingsFile {
    changed: Option<SystemTime>,
    models: Option<HashMap<String, ModelSettings>>,
}

impl OmlxModels {
    /// Every model's settings in `path`, read again only when `recheck`
    /// finds the file changed.
    fn model_settings(
        &mut self,
        host: &dyn Host,
        path: &Path,
        recheck: bool,
    ) -> Option<&HashMap<String, ModelSettings>> {
        if self.model_settings.is_none() || recheck {
            let changed = host.modified(path);
            if self
                .model_settings
                .as_ref()
                .is_none_or(|file| file.changed != changed)
            {
                let models = match changed {
                    Some(_) => host
                        .read_prefix(path, 4 * 1024 * 1024)
                        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                        .as_ref()
                        .and_then(parse_model_settings),
                    // Without the file oMLX runs every model on its defaults: its own
                    // engine, no draft. A base directory that cannot be read says nothing.
                    None => path
                        .parent()
                        .filter(|base| {
                            host.read_dir(base).iter().any(|entry| {
                                entry
                                    .file_name()
                                    .is_some_and(|name| name == "settings.json")
                            })
                        })
                        .map(|_| HashMap::new()),
                };
                self.model_settings = Some(SettingsFile { changed, models });
            }
        }
        self.model_settings.as_ref()?.models.as_ref()
    }
}

/// What oMLX's global settings say about serving: its base directory,
/// which holds `model_settings.json`, the Splash engine's KV format and
/// version, and how many requests it is set to run at once. Nothing else
/// is kept, the API key least of all.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct OmlxSettings {
    pub(crate) base_path: Option<PathBuf>,
    pub(crate) splash_kv_format: Option<String>,
    pub(crate) splash_version: Option<String>,
    pub(crate) max_concurrent_requests: Option<u64>,
}

impl OmlxSettings {
    /// From `/admin/api/global-settings`.
    pub(crate) fn parse(settings: &Value) -> Self {
        let text = |path: &[&str]| json_string(settings, path).map(|text| printable(&text));
        Self {
            base_path: json_string(settings, &["base_path"]).map(PathBuf::from),
            splash_kv_format: text(&["integrations", "splash_kv_format"]),
            splash_version: text(&["external_engines", "splash", "version"]),
            max_concurrent_requests: json_u64(settings, &["scheduler", "max_concurrent_requests"])
                .filter(|limit| *limit > 0),
        }
    }
}

/// One model as `/v1/models/status` lists it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ModelStatus {
    pub(crate) id: String,
    /// Where its files are.
    pub(crate) path: Option<PathBuf>,
    /// The engine serving it when not oMLX's own: `splash`.
    pub(crate) engine: Option<String>,
    /// The context oMLX allows (lowered to what Splash planned), and the
    /// context the model's config declares.
    pub(crate) context: Option<u64>,
    pub(crate) trained: Option<u64>,
}

pub(crate) fn parse_models_status(status: &Value) -> Vec<ModelStatus> {
    status
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            Some(ModelStatus {
                id: json_string(model, &["id"])?,
                path: json_string(model, &["model_path"])
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute()),
                engine: json_string(model, &["external_engine"]),
                context: json_u64(model, &["max_context_window"]).filter(|tokens| *tokens > 0),
                trained: json_u64(model, &["model_context_length"]).filter(|tokens| *tokens > 0),
            })
        })
        .collect()
}

/// One model's own settings in oMLX's `model_settings.json`; a model
/// without an entry runs with these defaults.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ModelSettings {
    /// `splash` when the model is to be served through Splash.
    pub(crate) engine: Option<String>,
    /// The KV cache's quantized format on oMLX's own engine: `TurboQuant
    /// 8-bit`, `affine 4-bit`; `None` when it is not quantized.
    pub(crate) kv_quantized: Option<String>,
    /// Speculative decoding on oMLX's own engine: the model's MTP head, a
    /// DFlash draft (`draft`, the configured checkpoint), or for a vision
    /// model an assistant model's MTP.
    pub(crate) mtp: bool,
    pub(crate) dflash: bool,
    pub(crate) draft: Option<PathBuf>,
    pub(crate) vlm_mtp: bool,
}

/// Each model's settings in `model_settings.json` (`{"models": {id: …}}`).
pub(crate) fn parse_model_settings(file: &Value) -> Option<HashMap<String, ModelSettings>> {
    let models = file.get("models")?.as_object()?;
    Some(
        models
            .iter()
            .map(|(id, model)| {
                let on = |key: &str| model.get(key).and_then(Value::as_bool) == Some(true);
                let text = |key: &str| model.get(key).and_then(Value::as_str);
                let kv_quantized = on("turboquant_kv_enabled").then(|| {
                    let bits = model
                        .get("turboquant_kv_bits")
                        .and_then(Value::as_f64)
                        .unwrap_or(4.0);
                    match text("turboquant_kv_scheme").unwrap_or("turboquant") {
                        "turboquant" => format!("TurboQuant {bits}-bit"),
                        scheme => {
                            printable(scheme.trim_end_matches(char::is_numeric))
                                + &format!(" {bits}-bit")
                        }
                    }
                });
                (
                    id.clone(),
                    ModelSettings {
                        engine: text("external_engine").map(printable),
                        kv_quantized,
                        mtp: on("mtp_enabled"),
                        dflash: on("dflash_enabled"),
                        // A configured draft is a path; anything else is
                        // resolved by oMLX in ways not repeated here.
                        draft: text("dflash_draft_model")
                            .map(PathBuf::from)
                            .filter(|path| path.is_absolute()),
                        vlm_mtp: on("vlm_mtp_enabled"),
                    },
                )
            })
            .collect(),
    )
}

/// Whether Splash serves the model: as its status says while it is loaded,
/// else as its settings say.
fn through_splash(status: Option<&ModelStatus>, settings: Option<&ModelSettings>) -> bool {
    status
        .and_then(|status| status.engine.as_deref())
        .or_else(|| settings.and_then(|settings| settings.engine.as_deref()))
        == Some("splash")
}

/// Everything known about how oMLX serves one model.
pub(crate) struct Serving<'a> {
    pub(crate) status: Option<&'a ModelStatus>,
    pub(crate) settings: &'a OmlxSettings,
    /// `None` when `model_settings.json` could not be read; a model it
    /// does not list, or a missing file, means oMLX's defaults.
    pub(crate) model_settings: Option<&'a ModelSettings>,
    /// The model's files and the draft's, where they could be read.
    pub(crate) model: Option<&'a MlxModel>,
    pub(crate) draft: Option<&'a MlxModel>,
    /// oMLX's version.
    pub(crate) version: Option<&'a str>,
    /// The model's entry in the admin stats' `active_models`.
    pub(crate) loaded: Option<&'a Value>,
}

/// The model's facts: its files as its engine runs them, with the KV
/// format oMLX's settings give, the draft, the context limits and the
/// sizes oMLX reports.
pub(crate) fn omlx_model_facts(serving: &Serving) -> ModelFacts {
    let settings = serving.model_settings;
    let splash = through_splash(serving.status, settings);
    let kv = if splash {
        serving
            .settings
            .splash_kv_format
            .as_deref()
            .map(|format| match format {
                "int8" => KvFormat::SplashInt8,
                "bf16" => KvFormat::Bf16,
                other => KvFormat::Named(other.to_owned()),
            })
    } else {
        settings.map(|settings| {
            settings
                .kv_quantized
                .clone()
                .map_or(KvFormat::Activations, KvFormat::Named)
        })
    };
    let engine = if splash {
        Engine::Splash
    } else {
        Engine::MlxLm
    };
    let files = MlxModel::default();
    let mut facts = mlx_facts(serving.model.unwrap_or(&files), engine, kv.as_ref());
    facts.context_configured = serving.status.and_then(|status| status.context);
    facts.context_trained = facts
        .context_trained
        .or_else(|| serving.status.and_then(|status| status.trained));
    (facts.draft, facts.draft_bytes) = if splash {
        // Splash serves only with a DFlash draft, the configured one when
        // it fits the model.
        let fits = serving
            .draft
            .zip(serving.model)
            .filter(|(draft, model)| draft.drafts_for(model));
        (Some(true), fits.and_then(|(draft, _)| draft.weight_bytes))
    } else {
        match settings {
            None => (None, None),
            Some(settings) if settings.dflash => (
                Some(true),
                serving.draft.and_then(|draft| draft.weight_bytes),
            ),
            // An assistant model drafts for a vision model; its files are not read.
            Some(settings) if settings.vlm_mtp => (Some(true), None),
            // MTP drafts with the model's own head, where it has one.
            Some(settings) if settings.mtp => {
                match serving.model.and_then(|model| model.mtp_bytes) {
                    Some(0) => (Some(false), None),
                    Some(bytes) => (Some(true), Some(bytes)),
                    None => (None, None),
                }
            }
            Some(_) => (Some(false), None),
        }
    };
    let engine_version = serving
        .settings
        .splash_version
        .as_deref()
        .filter(|_| splash);
    facts.runtime_version = (serving.version.is_some() || engine_version.is_some()).then(|| {
        let mut version = "oMLX".to_owned();
        for (separator, part) in [(" ", serving.version), (" · Splash ", engine_version)] {
            if let Some(part) = part {
                version.push_str(separator);
                version.push_str(part);
            }
        }
        version
    });
    facts.loaded_bytes = serving
        .loaded
        .and_then(|model| json_u64(model, &["actual_size"]));
    facts.estimated_bytes = serving
        .loaded
        .and_then(|model| json_u64(model, &["estimated_size"]));
    facts
}

/// What Diagnostics says about the draft: why tokens per verify pass are missing where oMLX
/// runs the model through Splash. oMLX starts Splash on a port and with an API key of its own
/// and passes on none of its draft counters. Nothing once the figure is known, or with no
/// draft.
pub(crate) fn draft_lines(sample: &Sample) -> Vec<String> {
    let hosted = sample.llm_provider == "oMLX" && sample.llm_engine.as_deref() == Some("Splash");
    if hosted && sample.llm_tokens_per_pass.is_none() && sample.model.draft != Some(false) {
        vec!["Draft: tokens per verify pass unknown · oMLX keeps Splash's status private".into()]
    } else {
        Vec::new()
    }
}

/// The tokens each verify pass gives the request shown, for its output line: what the draft
/// adds to its speed. Only while it is the one request writing, since the counters are the
/// engine's; empty otherwise.
pub(crate) fn draft_note(sample: &Sample, request: &RequestUsage, live: bool) -> String {
    let writing = |request: &RequestUsage| {
        !request.completed && !request.queued && request.prefilled.is_none()
    };
    let alone = sample
        .llm_requests
        .iter()
        .filter(|request| writing(request))
        .count()
        == 1;
    match sample.llm_tokens_per_pass {
        Some(tokens) if live && alone && writing(request) && sample.llm_fresh() => {
            format!("{tokens:.1} tok/pass")
        }
        _ => String::new(),
    }
}

#[cfg(test)]
#[path = "tests/omlx.rs"]
mod tests;
