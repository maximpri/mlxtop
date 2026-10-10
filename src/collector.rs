// SPDX-License-Identifier: MIT
//! Combine host and provider observations into bounded histories and snapshots.
use crate::analysis::{classify, signal_summary, CorrelationEngine};
use crate::completion_log::read_llm_stats;
use crate::config::{Config, Thresholds, GPU_BUSY_ENTER_LOAD};
use crate::domain::{
    ChartMetric, ChartPoint, EventKind, LlmLogStats, LlmTelemetry, MetalTelemetry, Sample,
    SignalEvent, TelemetrySource, Tone, VmCounters, MIB,
};
use crate::formatting::{
    bytes, llm_generation_rate_label, llm_model_label, llm_prefill_rate_label,
    pressure_state_label, rate,
};
use crate::history::{
    baseline_tone, compression_tone, delta, paging_tone, push_history, push_history_with_tone,
    rate_bytes, signed_rate_bytes,
};
use crate::host::{now_clock, Host, Platform};
use crate::logging::{
    diagnostics_log, log_field, log_optional_f64, log_optional_u64, log_optional_u8,
};
use crate::omlx::LlmTelemetryClient;
use crate::platform::{
    linux_counters_for_rates, linux_metal_init, linux_page_size, linux_total_memory,
    macos_counters_for_rates, parse_metal_hardware, resident_memory_percent,
    sample_linux_gpu_thermal, sample_linux_memory, sample_macos_gpu_thermal, sample_macos_memory,
    sample_windows_gpu_thermal, sample_windows_memory,
};
use crate::processes::{annotate_process_pagein_rates, parse_processes, snapshot};
use crate::{host, operator_history, process_memory, providers, request_history};
use std::collections::VecDeque;
use std::env;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};
pub(crate) struct PreviousCounters {
    pub(crate) at: Instant,
    pub(crate) swapins: u64,
    pub(crate) swapouts: u64,
    pub(crate) compressions: u64,
    pub(crate) decompressions: u64,
    pub(crate) reactivations: u64,
    pub(crate) swap_used: u64,
}

#[derive(Clone)]
pub(crate) struct CollectorView {
    pub(crate) current: Sample,
    pub(crate) generation_history: VecDeque<ChartPoint>,
    pub(crate) prefill_history: VecDeque<ChartPoint>,
    pub(crate) cache_history: VecDeque<ChartPoint>,
    pub(crate) load_history: VecDeque<ChartPoint>,
    pub(crate) swap_history: VecDeque<ChartPoint>,
    pub(crate) compression_history: VecDeque<ChartPoint>,
    /// Which host counters exist; Linux has no compressor counters to chart.
    pub(crate) platform: Platform,
    pub(crate) gpu_history: VecDeque<ChartPoint>,
    pub(crate) signals: VecDeque<SignalEvent>,
    pub(crate) request_history: request_history::History,
    pub(crate) operator_history: operator_history::History,
}

#[derive(Clone)]
pub(crate) struct CacheCounters {
    pub(crate) provider: Option<String>,
    pub(crate) prompt_tokens: u64,
    pub(crate) cached_tokens: u64,
}

pub(crate) struct Collector {
    pub(crate) host: Box<dyn Host>,
    pub(crate) platform: Platform,
    pub(crate) home: Option<PathBuf>,
    pub(crate) page_size: u64,
    pub(crate) total_memory: u64,
    pub(crate) metal: MetalTelemetry,
    pub(crate) llm_client: LlmTelemetryClient,
    pub(crate) seen_requests: VecDeque<(String, u64)>,
    pub(crate) correlation: CorrelationEngine,
    pub(crate) previous: Option<PreviousCounters>,
    pub(crate) previous_llm_cache: Option<CacheCounters>,
    pub(crate) current: Sample,
    pub(crate) generation_history: VecDeque<ChartPoint>,
    pub(crate) prefill_history: VecDeque<ChartPoint>,
    pub(crate) cache_history: VecDeque<ChartPoint>,
    pub(crate) load_history: VecDeque<ChartPoint>,
    pub(crate) swap_history: VecDeque<ChartPoint>,
    pub(crate) compression_history: VecDeque<ChartPoint>,
    pub(crate) gpu_history: VecDeque<ChartPoint>,
    pub(crate) signals: VecDeque<SignalEvent>,
    pub(crate) request_history: request_history::History,
    pub(crate) operator_history: operator_history::History,
    pub(crate) history_limit: usize,
    pub(crate) thresholds: Thresholds,
}

impl Collector {
    pub(crate) fn new(history_limit: usize, config: Config) -> Self {
        Self::with_host(
            history_limit,
            config,
            host::system(),
            Platform::current(),
            crate::config::home_dir(),
        )
    }

    pub(crate) fn with_host(
        history_limit: usize,
        config: Config,
        host: Box<dyn Host>,
        platform: Platform,
        home: Option<PathBuf>,
    ) -> Self {
        let (total_memory, page_size, metal) = match platform {
            Platform::MacOs => {
                let total_memory = host
                    .command_u64("/usr/sbin/sysctl", &["-n", "hw.memsize"])
                    .unwrap_or(0);
                let mut metal = parse_metal_hardware(
                    &host
                        .command(
                            "/usr/sbin/ioreg",
                            &["-r", "-d", "1", "-w", "0", "-c", "IOAccelerator"],
                        )
                        .unwrap_or_default(),
                );
                metal.architecture = host
                    .command("/usr/sbin/sysctl", &["-n", "hw.machine"])
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty());
                metal.resource_limit = host
                    .command_u64("/usr/sbin/sysctl", &["-n", "iogpu.wired_limit_mb"])
                    .filter(|value| *value > 0)
                    .map(|value| value.saturating_mul(MIB));
                let page_size = host
                    .command_u64("/usr/sbin/sysctl", &["-n", "hw.pagesize"])
                    .unwrap_or(16_384);
                (total_memory, page_size, metal)
            }
            // Linux (and other non-macOS targets): read from /proc and /sys.
            Platform::Linux => (
                linux_total_memory(host.as_ref()),
                linux_page_size(host.as_ref()),
                linux_metal_init(host.as_ref()),
            ),
            Platform::Windows => (
                host.windows().map(|reading| reading.total).unwrap_or(0),
                4096,
                MetalTelemetry {
                    architecture: Some(env::consts::ARCH.into()),
                    ..MetalTelemetry::default()
                },
            ),
        };
        Self {
            llm_client: LlmTelemetryClient::from_config(&config, home.clone()),
            host,
            platform,
            home,
            page_size,
            total_memory,
            metal,
            seen_requests: VecDeque::new(),
            correlation: CorrelationEngine::default(),
            previous: None,
            previous_llm_cache: None,
            current: Sample::default(),
            generation_history: VecDeque::with_capacity(history_limit),
            prefill_history: VecDeque::with_capacity(history_limit),
            cache_history: VecDeque::with_capacity(history_limit),
            load_history: VecDeque::with_capacity(history_limit),
            swap_history: VecDeque::with_capacity(history_limit),
            compression_history: VecDeque::with_capacity(history_limit),
            gpu_history: VecDeque::with_capacity(history_limit),
            signals: VecDeque::with_capacity(8),
            request_history: request_history::History::default(),
            operator_history: operator_history::History::default(),
            history_limit,
            thresholds: Thresholds::from_config(&config),
        }
    }

    pub(crate) fn sample(&mut self) -> Sample {
        let sample_started = Instant::now();
        let now = sample_started;
        let mut sample = Sample {
            total_memory: self.total_memory,
            metal: self.metal.clone(),
            ..Sample::default()
        };

        let host = self.host.as_ref();
        let windows = (self.platform == Platform::Windows)
            .then(|| host.windows())
            .flatten();
        match self.platform {
            Platform::Windows => {
                sample_windows_memory(windows.as_ref(), &mut sample, self.thresholds)
            }
            Platform::MacOs => sample_macos_memory(host, &mut sample, self.page_size),
            Platform::Linux => sample_linux_memory(
                host,
                &mut sample,
                self.page_size,
                self.total_memory,
                self.thresholds,
            ),
        }
        let counters = match self.platform {
            Platform::MacOs => macos_counters_for_rates(host, self.page_size),
            Platform::Linux => linux_counters_for_rates(host),
            Platform::Windows => VmCounters::default(),
        };
        let process_elapsed = self
            .previous
            .as_ref()
            .map(|previous| now.duration_since(previous.at));

        if let Some(previous) = &self.previous {
            let elapsed = now.duration_since(previous.at);
            sample.swap_in = rate_bytes(
                delta(counters.swapins, previous.swapins),
                self.page_size,
                elapsed,
            );
            sample.swap_out = rate_bytes(
                delta(counters.swapouts, previous.swapouts),
                self.page_size,
                elapsed,
            );
            sample.compress = rate_bytes(
                delta(counters.compressions, previous.compressions),
                self.page_size,
                elapsed,
            );
            sample.decompress = rate_bytes(
                delta(counters.decompressions, previous.decompressions),
                self.page_size,
                elapsed,
            );
            sample.reactivated = rate_bytes(
                delta(counters.reactivations, previous.reactivations),
                self.page_size,
                elapsed,
            );
            sample.swap_growth = signed_rate_bytes(sample.swap_used, previous.swap_used, elapsed);
            sample.rate_ready = true;
        }
        self.previous = Some(PreviousCounters {
            at: now,
            swapins: counters.swapins,
            swapouts: counters.swapouts,
            compressions: counters.compressions,
            decompressions: counters.decompressions,
            reactivations: counters.reactivations,
            swap_used: sample.swap_used,
        });

        match self.platform {
            Platform::MacOs => sample_macos_gpu_thermal(host, &mut sample),
            Platform::Linux => sample_linux_gpu_thermal(host, &mut sample, &self.current.gpus),
            Platform::Windows => sample_windows_gpu_thermal(host, &mut sample, &self.current.gpus),
        }

        let process_snapshot = match self.platform {
            Platform::MacOs => parse_processes(
                &host
                    .command(
                        "/bin/ps",
                        &["-axo", "pid=,rss=,%cpu=,%mem=,state=,pagein=,comm=,args="],
                    )
                    .unwrap_or_default(),
            ),
            // Linux `ps` has no `pagein` column; `maj_flt` (major faults)
            // keeps the same positional layout for the shared parser.
            Platform::Linux => parse_processes(
                &host
                    .command(
                        "ps",
                        &["-axo", "pid=,rss=,%cpu=,%mem=,stat=,maj_flt=,comm=,args="],
                    )
                    .unwrap_or_default(),
            ),
            Platform::Windows => {
                snapshot(windows.map(|reading| reading.processes).unwrap_or_default())
            }
        };
        sample.llm_count = process_snapshot.llm_count;
        sample.llm_rss = process_snapshot.llm_rss;
        sample.llm_cpu = process_snapshot.llm_cpu;
        let detected_provider = process_snapshot.provider.clone();
        sample.llm_pid = process_snapshot
            .top_llm
            .as_ref()
            .map(|process| process.pid)
            .unwrap_or_default();
        sample.process_memory = process_memory::read(sample.llm_pid);
        sample.process_memory_growth = sample.process_memory.as_ref().and_then(|reading| {
            process_memory::growth(reading, self.current.process_memory.as_ref())
        });
        sample.llm_top = process_snapshot
            .top_llm
            .as_ref()
            .map(|process| process.name.clone())
            .unwrap_or_else(|| "none".into());
        sample.largest_consumer = process_snapshot.largest_consumer;
        let mut llm_processes = process_snapshot.llm_processes;
        if let Some(elapsed) = process_elapsed {
            annotate_process_pagein_rates(&mut llm_processes, &self.current.llm_processes, elapsed);
        }
        sample.llm_processes = llm_processes;
        let live_stats = self.llm_client.poll(detected_provider.as_deref());
        sample.runtime = self.llm_client.report();
        let should_read_log = !self
            .llm_client
            .provider_adapter
            .selected(detected_provider.as_deref())
            && live_stats.is_none()
            && detected_provider
                .as_deref()
                .map(|provider| provider == "oMLX")
                .unwrap_or(true);
        let log_stats = if should_read_log {
            read_llm_stats(self.home.as_deref())
        } else {
            LlmLogStats::default()
        };
        let llm_stats = live_stats.as_ref();
        sample.llm_remote = sample.runtime.remote || llm_stats.is_some_and(|stats| stats.remote);
        sample.llm_details = llm_stats.and_then(|stats| stats.details.clone());
        sample.mlx = llm_stats.map(|stats| stats.mlx.clone()).unwrap_or_default();
        sample.metal.resource_limit = sample.metal.resource_limit.or(sample.mlx.resource_limit);
        sample.llm_requests = llm_stats
            .map(|stats| stats.requests.clone())
            .unwrap_or_default();
        let live_is_stale = llm_stats
            .filter(|stats| stats.source == TelemetrySource::Live)
            .and_then(|stats| stats.observed_at)
            .and_then(|observed_at| SystemTime::now().duration_since(observed_at).ok())
            .is_some_and(|age| age > Duration::from_secs(5));
        let use_omlx_log = log_stats.model.is_some()
            && detected_provider
                .as_deref()
                .map(|provider| provider == "oMLX")
                .unwrap_or(true);
        sample.llm_provider = llm_stats
            .and_then(|stats| stats.provider.clone())
            .unwrap_or_else(|| {
                if let Some(provider) = self.llm_client.provider_adapter.provider() {
                    provider.to_owned()
                } else if use_omlx_log {
                    "oMLX".into()
                } else {
                    "none".into()
                }
            });
        sample.llm_status = if live_is_stale {
            "stale".into()
        } else {
            llm_stats
                .and_then(|stats| stats.status.clone())
                .unwrap_or_else(|| {
                    if use_omlx_log {
                        "last result".into()
                    } else if sample.llm_count > 0 {
                        "running".into()
                    } else {
                        "offline".into()
                    }
                })
        };
        sample.llm_model = llm_stats
            .and_then(|stats| stats.model.clone())
            .or_else(|| use_omlx_log.then(|| log_stats.model.clone()).flatten())
            .unwrap_or_else(|| {
                if sample.llm_count > 0 {
                    sample.llm_top.clone()
                } else {
                    "not detected".into()
                }
            });
        sample.llm_source = live_stats
            .as_ref()
            .map(|stats| stats.source)
            .unwrap_or_else(|| {
                if use_omlx_log {
                    TelemetrySource::Log
                } else {
                    TelemetrySource::None
                }
            });
        sample.llm_observed_at = live_stats
            .as_ref()
            .and_then(|stats| stats.observed_at)
            .or_else(|| use_omlx_log.then_some(log_stats.observed_at).flatten());
        sample.llm_generation_tps =
            llm_stats
                .and_then(|stats| stats.generation_tps)
                .or_else(|| {
                    if live_stats.is_none() && use_omlx_log {
                        log_stats.tokens_per_second
                    } else {
                        None
                    }
                });
        sample.llm_generation_tps_live = !live_is_stale
            && llm_stats
                .map(|stats| stats.generation_tps_live)
                .unwrap_or(false);
        sample.llm_prefill_tps = llm_stats.and_then(|stats| stats.prefill_tps);
        sample.llm_prefill_tps_live = !live_is_stale
            && llm_stats
                .map(|stats| stats.prefill_tps_live)
                .unwrap_or(false);
        sample.llm_output_tokens = llm_stats.and_then(|stats| stats.output_tokens).or_else(|| {
            if live_stats.is_none() && use_omlx_log {
                log_stats.output_tokens
            } else {
                None
            }
        });
        sample.llm_prompt_tokens = llm_stats.and_then(|stats| stats.prompt_tokens).or_else(|| {
            if live_stats.is_none() && use_omlx_log {
                log_stats.prompt_tokens
            } else {
                None
            }
        });
        sample.llm_cache_efficiency = llm_stats.and_then(|stats| stats.cache_efficiency);
        sample.llm_cache_interval_efficiency =
            cache_interval_efficiency(&mut self.previous_llm_cache, llm_stats, live_is_stale)
                .or_else(|| {
                    (!live_is_stale)
                        .then(|| llm_stats.and_then(|s| s.cache_interval_efficiency))
                        .flatten()
                });
        sample.llm_prefix_hit_rate = llm_stats.and_then(|stats| stats.prefix_hit_rate);
        sample.llm_active_requests = llm_stats.and_then(|stats| stats.active_requests);
        sample.llm_waiting_requests = llm_stats.and_then(|stats| stats.waiting_requests);
        sample.llm_model_memory = llm_stats.and_then(|stats| stats.model_memory);
        sample.llm_model_memory_max = llm_stats.and_then(|stats| stats.model_memory_max);
        sample.llm_model_offloaded = llm_stats.and_then(|stats| stats.model_offloaded);
        sample.llm_model_size = llm_stats.and_then(|stats| stats.model_size);
        (sample.updated, sample.utc_offset) = now_clock(self.host.as_ref(), self.platform);
        let previous = if self.current.updated == "waiting" {
            None
        } else {
            Some(self.current.clone())
        };
        sample.correlation = self.correlation.observe(&sample, self.thresholds);
        classify(&mut sample, previous.as_ref(), self.thresholds);

        if previous.as_ref().is_some_and(|old| {
            old.llm_provider != sample.llm_provider || old.llm_model != sample.llm_model
        }) {
            self.generation_history.clear();
            self.prefill_history.clear();
        }

        push_history(
            &mut self.generation_history,
            chart_rate_value(&sample, ChartMetric::Generation),
            ChartMetric::Generation,
            self.history_limit,
            self.thresholds,
        );
        // Prefill has no fixed threshold: grade against its own rolling baseline.
        let prefill = chart_rate_value(&sample, ChartMetric::Prefill);
        let prefill_tone = prefill
            .map(|value| baseline_tone(value, &self.prefill_history))
            .unwrap_or(Tone::Muted);
        push_history_with_tone(
            &mut self.prefill_history,
            prefill,
            prefill_tone,
            self.history_limit,
        );
        push_history(
            &mut self.cache_history,
            sample
                .llm_cache_interval_efficiency
                .filter(|value| value.is_finite() && *value >= 0.0)
                .map(|value| value.round() as u64),
            ChartMetric::Cache,
            self.history_limit,
            self.thresholds,
        );
        if let Some(point) = self.generation_history.back_mut() {
            point.tone = if point.value.is_none() {
                Tone::Muted
            } else if sample.correlation.is_material_drop() {
                // Graded against the rolling baseline, like the assessment.
                sample.correlation.tone()
            } else {
                Tone::Green
            };
        }
        push_history_with_tone(
            &mut self.load_history,
            resident_memory_percent(&sample),
            sample.pressure_tone,
            self.history_limit,
        );
        // Captured tones follow the assessment's states, so a chart never
        // shows a severity the headline does not report.
        let paging = sample
            .paging_measured()
            .then_some(sample.swap_in.saturating_add(sample.swap_out));
        push_history_with_tone(
            &mut self.swap_history,
            paging,
            paging
                .map(|churn| paging_tone(churn, &sample.impact, self.thresholds))
                .unwrap_or(Tone::Muted),
            self.history_limit,
        );
        let compression = (sample.vm_available && sample.rate_ready)
            .then_some(sample.compress.saturating_add(sample.decompress));
        let previous_tone = self
            .compression_history
            .back()
            .filter(|point| point.value.is_some())
            .map(|point| point.tone);
        push_history_with_tone(
            &mut self.compression_history,
            compression,
            compression
                .map(|churn| compression_tone(churn, previous_tone, self.thresholds))
                .unwrap_or(Tone::Muted),
            self.history_limit,
        );
        push_history(
            &mut self.gpu_history,
            sample.gpu_util.map(u64::from),
            ChartMetric::Gpu,
            self.history_limit,
            self.thresholds,
        );

        self.record_journal_events(previous.as_ref(), &sample);
        diagnostics_log(
            "INFO",
            "sample",
            format!(
                "duration_ms={} status={} provider={} model={} source={} gen_live={} gen_tps={} prefill_live={} prefill_tps={} cache={} active={} waiting={} gpu={} renderer={} tiler={} ram_resident_percent={} vm_availability={} paging_in={} paging_out={} compress={} decompress={}",
                sample_started.elapsed().as_millis(),
                log_field(&sample.llm_status),
                log_field(&sample.llm_provider),
                log_field(&sample.llm_model),
                log_field(sample.llm_source.label()),
                sample.llm_generation_tps_live,
                log_optional_f64(sample.llm_generation_tps),
                sample.llm_prefill_tps_live,
                log_optional_f64(sample.llm_prefill_tps),
                log_optional_f64(sample.llm_cache_efficiency),
                log_optional_u64(sample.llm_active_requests),
                log_optional_u64(sample.llm_waiting_requests),
                log_optional_u8(sample.gpu_util),
                log_optional_u8(sample.metal.renderer_util),
                log_optional_u8(sample.metal.tiler_util),
                log_optional_u64(resident_memory_percent(&sample)),
                log_optional_u8(sample.availability),
                sample.swap_in,
                sample.swap_out,
                sample.compress,
                sample.decompress,
            ),
        );
        self.current = sample.clone();
        sample
    }

    pub(crate) fn view(&self) -> CollectorView {
        CollectorView {
            current: self.current.clone(),
            generation_history: self.generation_history.clone(),
            prefill_history: self.prefill_history.clone(),
            cache_history: self.cache_history.clone(),
            load_history: self.load_history.clone(),
            swap_history: self.swap_history.clone(),
            compression_history: self.compression_history.clone(),
            platform: self.platform,
            gpu_history: self.gpu_history.clone(),
            signals: self.signals.clone(),
            request_history: self.request_history.clone(),
            operator_history: self.operator_history.clone(),
        }
    }

    pub(crate) fn record_journal_events(&mut self, previous: Option<&Sample>, sample: &Sample) {
        let mut events = Vec::new();
        let mut add = |state: &str, summary: String, tone: Tone| {
            events.push((state.to_string(), summary, tone));
        };

        self.request_history.observe(&sample.llm_requests);
        self.operator_history.observe(sample, self.history_limit);
        for request in &sample.llm_requests {
            if let Some(summary) = providers::new_request_summary(&mut self.seen_requests, request)
            {
                add("PROMPT", summary, Tone::Cyan);
            }
        }

        if let Some(previous) = previous {
            if previous.impact != sample.impact && sample.impact != "SAMPLING" {
                add(&sample.impact, signal_summary(sample), sample.impact_tone);
            }

            if (previous.llm_provider != sample.llm_provider
                || previous.llm_model != sample.llm_model)
                && sample.llm_provider != "none"
                && sample.llm_model != "not detected"
            {
                add(
                    "LLM",
                    format!(
                        "detected {} · {}",
                        sample.llm_provider,
                        llm_model_label(sample, 42)
                    ),
                    Tone::Cyan,
                );
            }

            if previous.llm_status != sample.llm_status {
                add(
                    "LLM",
                    format!(
                        "status {} → {} · {}",
                        previous.llm_status, sample.llm_status, sample.llm_provider
                    ),
                    Tone::Cyan,
                );
            }

            if previous.llm_source != sample.llm_source {
                add(
                    "LLM",
                    format!(
                        "telemetry source · {} → {}",
                        previous.llm_source.label(),
                        sample.llm_source.label()
                    ),
                    if sample.llm_source == TelemetrySource::Live {
                        Tone::Green
                    } else {
                        Tone::Yellow
                    },
                );
            }

            let previous_active = previous.llm_active_requests.unwrap_or(0);
            let active = sample.llm_active_requests.unwrap_or(0);
            if (previous_active == 0) != (active == 0) {
                add(
                    "LLM",
                    if active == 0 {
                        "request completed · serving is idle".into()
                    } else {
                        format!("request started · {active} active")
                    },
                    Tone::Cyan,
                );
            }

            let previous_waiting = previous.llm_waiting_requests.unwrap_or(0);
            let waiting = sample.llm_waiting_requests.unwrap_or(0);
            if (previous_waiting == 0) != (waiting == 0) {
                add(
                    "QUEUE",
                    if waiting == 0 {
                        "queue cleared".into()
                    } else {
                        format!("{waiting} request(s) waiting")
                    },
                    if waiting == 0 {
                        Tone::Green
                    } else {
                        Tone::Yellow
                    },
                );
            }

            if previous.llm_generation_tps.is_none() && sample.llm_generation_tps.is_some() {
                add(
                    "LLM",
                    format!(
                        "{} telemetry online · {} · {}",
                        sample.llm_source.label(),
                        llm_generation_rate_label(sample),
                        llm_prefill_rate_label(sample)
                    ),
                    Tone::Green,
                );
            }

            if sample.correlation.is_material_drop()
                && previous.correlation.event_key != sample.correlation.event_key
            {
                add(
                    "LLM",
                    format!(
                        "throughput diagnosis · {} · {} confidence",
                        sample.correlation.summary,
                        sample.correlation.confidence_label()
                    ),
                    sample.correlation.tone(),
                );
            }

            if previous.pressure != sample.pressure && !sample.pressure.is_empty() {
                add(
                    "PRESSURE",
                    format!("memory state {}", pressure_state_label(sample)),
                    sample.pressure_tone,
                );
            }

            let previous_paging = previous.swap_in.saturating_add(previous.swap_out) > 0;
            let paging = sample.swap_in.saturating_add(sample.swap_out) > 0;
            if previous_paging != paging {
                // Same band as the paging chart: traffic below the warning
                // rate is light paging, not a condition worth inspecting.
                let churn = sample.swap_in.saturating_add(sample.swap_out);
                let tone = ChartMetric::Swap.tone(churn, self.thresholds);
                add(
                    "PAGING",
                    if paging {
                        format!(
                            "{} · in {} · out {}",
                            if tone == Tone::Green {
                                "light"
                            } else {
                                "active"
                            },
                            rate(sample.swap_in),
                            rate(sample.swap_out)
                        )
                    } else {
                        "cleared · no current paging traffic".into()
                    },
                    if paging { tone } else { Tone::Green },
                );
            }

            if let Some((summary, tone)) =
                gpu_journal_transition(previous.gpu_util, sample.gpu_util, self.thresholds)
            {
                add("GPU", summary, tone);
            }

            if previous.thermal != sample.thermal
                && sample.thermal != "unavailable"
                && !sample.thermal.ends_with("°C measured")
            {
                add(
                    "THERMAL",
                    sample.thermal.clone(),
                    if sample.thermal == "no warning" {
                        Tone::Green
                    } else {
                        Tone::Yellow
                    },
                );
            }
        } else {
            add(
                "SYSTEM",
                format!(
                    "journal started · RAM resident {} · pressure {}",
                    sample
                        .resident_memory
                        .map(bytes)
                        .unwrap_or_else(|| "—".into()),
                    pressure_state_label(sample)
                ),
                Tone::Cyan,
            );
            if sample.llm_provider != "none" && sample.llm_model != "not detected" {
                add(
                    "LLM",
                    format!(
                        "detected {} · {}",
                        sample.llm_provider,
                        llm_model_label(sample, 42)
                    ),
                    Tone::Cyan,
                );
            }
        }

        for (state, summary, tone) in events {
            self.signals.push_back(SignalEvent {
                time: sample.updated.clone(),
                recorded_at: SystemTime::now(),
                kind: EventKind::from_state(&state),
                state,
                summary,
                tone,
            });
        }
        let journal_limit = self.history_limit.clamp(40, 240);
        while self.signals.len() > journal_limit {
            self.signals.pop_front();
        }
    }

    pub(crate) fn reset(&mut self) {
        self.previous = None;
        self.previous_llm_cache = None;
        self.seen_requests.clear();
        self.request_history = request_history::History::default();
        self.operator_history = operator_history::History::default();
        self.correlation.reset();
        self.generation_history.clear();
        self.prefill_history.clear();
        self.cache_history.clear();
        self.load_history.clear();
        self.swap_history.clear();
        self.compression_history.clear();
        self.gpu_history.clear();
        self.signals.clear();
        self.current = Sample::default();
    }
}

/// Return a live request rate for a chart. Aggregate session rates and
/// completion-log values are deliberately excluded because they are not
/// measurements of the current sample.
pub(crate) fn chart_rate_value(sample: &Sample, metric: ChartMetric) -> Option<u64> {
    let (value, live) = match metric {
        ChartMetric::Generation => (sample.llm_generation_tps, sample.llm_generation_tps_live),
        ChartMetric::Prefill => (sample.llm_prefill_tps, sample.llm_prefill_tps_live),
        _ => return None,
    };
    if !live {
        return None;
    }
    value
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| (value * 10.0).round() as u64)
}

pub(crate) fn cache_interval_efficiency(
    previous: &mut Option<CacheCounters>,
    telemetry: Option<&LlmTelemetry>,
    stale: bool,
) -> Option<f64> {
    if stale {
        *previous = None;
        return None;
    }
    let Some(telemetry) = telemetry else {
        *previous = None;
        return None;
    };
    let Some(counters) = telemetry
        .total_prompt_tokens
        .zip(telemetry.total_cached_tokens)
        .map(|(prompt_tokens, cached_tokens)| CacheCounters {
            provider: telemetry.provider.clone(),
            prompt_tokens,
            cached_tokens,
        })
    else {
        *previous = None;
        return None;
    };
    let previous = previous.replace(counters.clone())?;
    if previous.provider != telemetry.provider {
        return None;
    }
    let prompt_delta = counters.prompt_tokens.checked_sub(previous.prompt_tokens)?;
    if prompt_delta == 0 {
        return None;
    }
    let cached_delta = counters.cached_tokens.checked_sub(previous.cached_tokens)?;
    if cached_delta > prompt_delta {
        return None;
    }
    Some(cached_delta as f64 / prompt_delta as f64 * 100.0)
}

/// Report measured GPU load transitions without declaring a critical fault.
/// Missing readings cannot establish that a busy period ended.
pub(crate) fn gpu_journal_transition(
    previous: Option<u8>,
    current: Option<u8>,
    thresholds: Thresholds,
) -> Option<(String, Tone)> {
    let (previous, current) = (u64::from(previous?), u64::from(current?));
    let saturated = current >= thresholds.gpu_critical_load;
    let saturation_changed = (previous >= thresholds.gpu_critical_load) != saturated;
    // Keep the existing busy-burst threshold to avoid logging every 75↔74%
    // fluctuation. Per-sample chart bands still retain all measured changes.
    let busy = current >= GPU_BUSY_ENTER_LOAD;
    let busy_changed = (previous >= GPU_BUSY_ENTER_LOAD) != busy;
    if !saturation_changed && !busy_changed {
        return None;
    }
    let label = if current == 0 {
        "GPU idle"
    } else if saturation_changed && saturated {
        "GPU load saturated"
    } else if !saturation_changed && busy {
        "busy burst started"
    } else {
        "GPU load eased"
    };
    Some((
        format!("{label} · {current}% busy"),
        ChartMetric::Gpu.tone(current, thresholds),
    ))
}
