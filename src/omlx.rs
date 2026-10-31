// SPDX-License-Identifier: MIT
//! oMLX monitoring, login, endpoint discovery and telemetry normalization.
use crate::config::{Config, DEFAULT_OMLX_HOST, DEFAULT_OMLX_PORT};
use crate::domain::{LlmTelemetry, MlxTelemetry, TelemetrySource};
use crate::json::{
    gib_to_bytes, json_f64, json_f64_key, json_string, json_string_paths_or_keys, json_u64,
    json_u64_paths_or_keys,
};
use crate::logging::{diagnostics_log, log_field};
use crate::providers;
use crate::runtime_diagnostics::{ProbeIssue, RuntimeReport};
use crate::transport::{http_request, HttpResponse};
use serde_json::{json, Value};
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
    model_metadata: Option<Value>,
    pub(crate) last_stats_available: Option<bool>,
    pub(crate) next_metadata_poll: Instant,
    pub(crate) next_poll: Instant,
    pub(crate) retry_backoff: Duration,
    pub(crate) runtime: RuntimeReport,
    using_adapter: bool,
    configured_endpoint: bool,
}

impl LlmTelemetryClient {
    pub(crate) fn from_config(config: &Config, home: Option<PathBuf>) -> Self {
        let (host, port) = read_omlx_endpoint(config, home.as_deref());
        Self {
            provider_adapter: providers::Adapter::new(),
            home,
            host,
            port,
            session_cookie: None,
            cached: None,
            mlx_metadata: MlxTelemetry::default(),
            model_metadata: None,
            last_stats_available: None,
            next_metadata_poll: Instant::now(),
            next_poll: Instant::now(),
            retry_backoff: Duration::from_secs(1),
            runtime: RuntimeReport::default(),
            using_adapter: false,
            configured_endpoint: config
                .omx
                .as_ref()
                .is_some_and(|omx| omx.host.is_some() || omx.port.is_some()),
        }
    }

    pub(crate) fn poll(&mut self, detected_provider: Option<&str>) -> Option<LlmTelemetry> {
        self.using_adapter = self.provider_adapter.selected(detected_provider);
        if self.using_adapter {
            return self.provider_adapter.poll();
        }
        let now = Instant::now();
        if now < self.next_poll {
            return self.cached.clone();
        }
        self.runtime.provider = self
            .provider_adapter
            .provider()
            .map(str::to_owned)
            .or_else(|| self.configured_endpoint.then(|| "oMLX".into()))
            .or_else(|| self.runtime.provider.clone());
        self.runtime.selection = if self.provider_adapter.explicitly_selected() {
            "MLXTOP_PROVIDER; oMLX endpoint config"
        } else if self.configured_endpoint {
            "oMLX endpoint config"
        } else if detected_provider.is_some() {
            "process detection; oMLX endpoint config"
        } else {
            "automatic oMLX discovery"
        };
        let authority = if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        self.runtime.endpoint =
            crate::runtime_diagnostics::safe_endpoint(&format!("http://{authority}:{}", self.port))
                .unwrap_or_else(|| "<invalid endpoint>".into());
        self.runtime.remote = !is_loopback_host(&self.host);
        self.runtime.credentials_present = read_omlx_api_key(self.home.as_deref()).is_some();
        self.runtime.begin();
        let telemetry = if self.runtime.endpoint == "<invalid endpoint>" {
            self.runtime
                .record("configuration", Err(ProbeIssue::InvalidEndpoint))
        } else {
            self.poll_once()
        };
        self.runtime.finish(telemetry.is_some());
        if telemetry.is_some() {
            self.runtime.provider = Some("oMLX".into());
        }

        if let Some(mut telemetry) = telemetry {
            telemetry.remote = !is_loopback_host(&self.host);
            self.cached = Some(telemetry);
            self.retry_backoff = Duration::from_secs(1);
            self.next_poll = now + self.retry_backoff;
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
            self.next_poll = now + self.retry_backoff;
            self.retry_backoff = (self.retry_backoff * 2).min(Duration::from_secs(30));
        }
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
        if now >= self.next_metadata_poll {
            self.model_metadata = self.fetch_json("/admin/api/models");
            let device_info = self.fetch_json("/admin/api/device-info");
            let settings = self
                .fetch_json("/admin/api/global-settings")
                .or_else(|| self.fetch_json("/admin/api/settings"));
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
        telemetry.mlx = merge_mlx_telemetry(
            &self.mlx_metadata,
            &parse_mlx_runtime_telemetry(&health, stats.as_ref()),
        );
        if let Some(stats) = &stats {
            crate::runtime_features::append_details(
                &mut telemetry.details,
                crate::runtime_features::omlx_details(stats, self.model_metadata.as_ref()),
            );
        }
        telemetry.observed_at = Some(SystemTime::now());
        Some(telemetry)
    }

    pub(crate) fn fetch_stats(&mut self) -> Option<Value> {
        self.fetch_json("/admin/api/stats?scope=session")
    }

    pub(crate) fn fetch_json(&mut self, path: &str) -> Option<Value> {
        if self.session_cookie.is_none() {
            self.login();
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
        let Some(api_key) = read_omlx_api_key(self.home.as_deref()) else {
            self.runtime
                .record::<()>("/admin/api/login", Err(ProbeIssue::CredentialsMissing));
            return;
        };
        let body = json!({ "api_key": api_key, "remember": true }).to_string();
        let Some(response) = self.request(
            "POST",
            "/admin/api/login",
            &[("Content-Type", "application/json")],
            Some(&body),
        ) else {
            return;
        };
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
 * Endpoint values discovered from `~/.config/omlx-coding/server.env`.
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
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
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

pub(crate) fn read_omlx_endpoint(config: &Config, home: Option<&Path>) -> (String, u16) {
    let discovered = home
        .map(|home| home.join(".config/omlx-coding/server.env"))
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|text| parse_omlx_server_env(&text))
        .unwrap_or_default();
    let native = crate::provider_discovery::omlx_settings(home);
    let discovered = DiscoveredEndpoint {
        host: discovered.host.or_else(|| {
            native
                .as_ref()?
                .pointer("/server/host")?
                .as_str()
                .map(crate::provider_discovery::local_bind)
        }),
        port: discovered.port.or_else(|| {
            native
                .as_ref()?
                .pointer("/server/port")?
                .as_u64()?
                .try_into()
                .ok()
                .filter(|p| *p > 0)
        }),
    };
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

pub(crate) fn read_omlx_api_key(home: Option<&Path>) -> Option<String> {
    env::var("MLXTOP_PROVIDER_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let text = fs::read_to_string(home?.join(".config/omlx-coding/server.env")).ok()?;
            text.lines()
                .find_map(|line| {
                    let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
                    let (key, value) = line.split_once('=')?;
                    (key.trim() == "API_KEY").then(|| value.trim().trim_matches('"').to_owned())
                })
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            crate::provider_discovery::omlx_settings(home)?
                .pointer("/auth/api_key")?
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .filter(|s| !s.chars().any(char::is_control))
}

pub(crate) fn parse_omlx_telemetry(health: &Value, stats: Option<&Value>) -> LlmTelemetry {
    let mut telemetry = LlmTelemetry {
        source: TelemetrySource::Live,
        provider: Some("oMLX".into()),
        status: Some(json_string(health, &["status"]).unwrap_or_else(|| "healthy".into())),
        model: json_string(health, &["default_model"]),
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
    if let [request] = prefilling.as_slice() {
        telemetry.prefill_tokens_done = request.processed;
        telemetry.prefill_tokens_expected = request.prompt.filter(|n| *n > 0);
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
    if let Some(model_cache) = model_cache {
        telemetry.prefix_hit_rate = json_f64(
            model_cache,
            &["cache_rates", "cumulative", "prefix_hit_rate"],
        )
        .map(|value| (value * 100.0).clamp(0.0, 100.0));
    }
    telemetry
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
