// SPDX-License-Identifier: MIT
//! Background collection and commands; the input/render thread never polls providers.
//! Under `mlxtop serve` there is no screen: each sample's snapshot goes to
//! the exporter's slot instead, and a request never waits for a sample.
use crate::collector::{Collector, CollectorView};
use crate::config::Config;
use crate::exporter::Latest;
use crate::logging::{diagnostics_log, log_field, panic_payload};
use std::panic::AssertUnwindSafe;
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender, SyncSender};
use std::time::{Duration, Instant};
use std::{panic, thread};
pub(crate) enum SamplerCommand {
    SetPaused(bool),
    SetInterval(Duration),
    /// A shorter history drops the oldest samples on the next sample.
    SetHistory(usize),
    /// Settings saved a new config: new thresholds, and a new endpoint if `omx` changed.
    SetConfig(Box<Config>),
    Reset,
    Stop,
}

pub(crate) struct Sampler {
    pub(crate) commands: Sender<SamplerCommand>,
    pub(crate) views: Receiver<CollectorView>,
    pub(crate) handle: Option<thread::JoinHandle<()>>,
}

/// Where each sample goes.
enum Output {
    /// The dashboard's one-slot channel: the collector and every history,
    /// for the screen to draw.
    Views(SyncSender<CollectorView>),
    /// `mlxtop serve`'s slot: the sample's JSON snapshot alone, for the
    /// exporter to answer from.
    Latest(Latest),
}

impl Sampler {
    pub(crate) fn spawn(interval: Duration, history_limit: usize, config: Config) -> Self {
        diagnostics_log(
            "INFO",
            "sampler_start",
            format!(
                "interval_seconds={} history_limit={history_limit}",
                interval.as_secs()
            ),
        );
        Self::start(interval, move || Collector::new(history_limit, config))
    }

    /// Run `build`'s collector on the sampling thread. The collector is built
    /// there because its first host reads may block on slow commands.
    pub(crate) fn start(
        interval: Duration,
        build: impl FnOnce() -> Collector + Send + 'static,
    ) -> Self {
        // Only the newest snapshot is ever used: a terminal that stops
        // reading must not queue them without limit.
        let (view_tx, view_rx) = mpsc::sync_channel(1);
        Self::run(interval, build, Output::Views(view_tx), view_rx)
    }

    /// Sample with no screen, for `mlxtop serve`: each sample's snapshot
    /// replaces the one in `latest`, and no view is built.
    pub(crate) fn headless(
        interval: Duration,
        build: impl FnOnce() -> Collector + Send + 'static,
        latest: Latest,
    ) -> Self {
        // Nothing draws, so nothing is ever sent on the view channel.
        let (_, views) = mpsc::sync_channel(1);
        Self::run(interval, build, Output::Latest(latest), views)
    }

    fn run(
        interval: Duration,
        build: impl FnOnce() -> Collector + Send + 'static,
        output: Output,
        views: Receiver<CollectorView>,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let mut collector = build();
            let mut interval = interval;
            crate::providers::set_poll_interval(interval);
            let mut paused = false;
            let mut next_sample = Instant::now();

            loop {
                if !paused && Instant::now() >= next_sample {
                    let started = Instant::now();
                    let sample = match panic::catch_unwind(AssertUnwindSafe(|| collector.sample()))
                    {
                        Ok(sample) => sample,
                        Err(payload) => {
                            diagnostics_log(
                                "ERROR",
                                "sampler_panic",
                                format!("message={}", log_field(&panic_payload(payload.as_ref()))),
                            );
                            break;
                        }
                    };
                    match &output {
                        Output::Views(view_tx) => match view_tx.try_send(collector.view()) {
                            // The interface has not taken the last one yet; the
                            // next snapshot is complete, so skipping loses nothing.
                            Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
                            Err(mpsc::TrySendError::Disconnected(_)) => {
                                diagnostics_log(
                                    "INFO",
                                    "sampler_stop",
                                    "reason=view_receiver_closed",
                                );
                                break;
                            }
                        },
                        Output::Latest(latest) => latest.publish(crate::export::snapshot(
                            &sample,
                            collector.platform,
                            interval.as_secs(),
                        )),
                    }
                    // A fixed rate: "every 1s" means a sample each second,
                    // not a second after the last one finished.
                    next_sample = (started + interval).max(Instant::now());
                }

                let wait = if paused {
                    Duration::from_millis(100)
                } else {
                    next_sample
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(100))
                };
                match command_rx.recv_timeout(wait) {
                    Ok(SamplerCommand::SetPaused(value)) => {
                        paused = value;
                        diagnostics_log("INFO", "sampler_paused", format!("paused={paused}"));
                        if !paused {
                            next_sample = Instant::now();
                        }
                    }
                    Ok(SamplerCommand::SetInterval(value)) => {
                        interval = value;
                        crate::providers::set_poll_interval(interval);
                        diagnostics_log(
                            "INFO",
                            "sampler_interval_changed",
                            format!("interval_seconds={}", interval.as_secs()),
                        );
                        next_sample = Instant::now() + interval;
                    }
                    Ok(SamplerCommand::SetHistory(limit)) => {
                        collector.history_limit = limit;
                        diagnostics_log(
                            "INFO",
                            "sampler_history_changed",
                            format!("history_limit={limit}"),
                        );
                    }
                    Ok(SamplerCommand::SetConfig(config)) => {
                        collector.apply_config(&config);
                        diagnostics_log("INFO", "sampler_config_changed", "source=settings");
                    }
                    Ok(SamplerCommand::Reset) => {
                        collector.reset();
                        diagnostics_log("INFO", "sampler_reset", "history_and_baselines_cleared");
                        next_sample = Instant::now();
                    }
                    Ok(SamplerCommand::Stop) => {
                        diagnostics_log("INFO", "sampler_stop", "reason=shutdown");
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        diagnostics_log("WARN", "sampler_stop", "reason=command_channel_closed");
                        break;
                    }
                }
            }
        });
        Self {
            commands: command_tx,
            views,
            handle: Some(handle),
        }
    }

    pub(crate) fn send(&self, command: SamplerCommand) {
        let _ = self.commands.send(command);
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        let _ = self.commands.send(SamplerCommand::Stop);
        if let Some(handle) = self.handle.take() {
            // A sample in flight may be waiting on a runtime: quit does not
            // wait for it; process exit ends the thread.
            let waited = Instant::now();
            while !handle.is_finished() && waited.elapsed() < Duration::from_millis(300) {
                thread::sleep(Duration::from_millis(5));
            }
            if !handle.is_finished() {
                diagnostics_log("INFO", "sampler_stop", "reason=abandoned_mid_sample");
                return;
            }
            if let Err(payload) = handle.join() {
                diagnostics_log(
                    "ERROR",
                    "sampler_thread_panic",
                    format!("message={}", log_field(&panic_payload(payload.as_ref()))),
                );
            }
        }
    }
}
