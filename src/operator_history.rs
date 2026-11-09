// SPDX-License-Identifier: MIT
//! Bounded OS/queue samples and explicitly reported request latency.
use crate::domain::{Sample, TelemetrySource};
use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

#[derive(Clone, Default)]
pub(crate) struct Point {
    pub(crate) active: Option<u64>,
    pub(crate) waiting: Option<u64>,
    pub(crate) provider: String,
}

/// Where a request is in the server: reading its prompt, writing its answer,
/// or waiting for a slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Phase {
    /// Prompt tokens processed so far.
    Prefill(u64),
    Generate,
    Queued,
}

/// One request in the server as the latest sample reported it.
#[derive(Clone)]
pub(crate) struct Running {
    pub(crate) id: String,
    pub(crate) prompt: u64,
    pub(crate) phase: Phase,
    pub(crate) output: Option<u64>,
    pub(crate) tps: Option<f64>,
}

#[derive(Clone, Default)]
pub(super) struct History {
    pub(crate) points: VecDeque<Point>,
    pub(crate) timings: VecDeque<(String, Option<u64>, SystemTime)>,
    /// The requests in the server now: prefilling first, then generating, then
    /// queued, each in the order the runtime lists them. Empty unless the
    /// runtime reports a speed per request.
    pub(crate) running: Vec<Running>,
    /// The runtime that has reported a request's own speed. Once it has, its
    /// requests stay listed while one has none, as Splash does with two
    /// running, so the queue doesn't switch views between samples.
    speeds_from: Option<String>,
}

impl History {
    pub fn observe(&mut self, sample: &Sample, limit: usize) {
        let fresh = sample.llm_source == TelemetrySource::Live
            && sample.llm_status != "stale"
            && sample.llm_observed_at.is_some_and(|at| {
                SystemTime::now()
                    .duration_since(at)
                    .is_ok_and(|age| age <= Duration::from_secs(5))
            });
        self.points.push_back(Point {
            active: fresh.then_some(sample.llm_active_requests).flatten(),
            waiting: fresh.then_some(sample.llm_waiting_requests).flatten(),
            provider: sample.llm_provider.clone(),
        });
        while self.points.len() > limit {
            self.points.pop_front();
        }
        let mut running: Vec<Running> = sample
            .llm_requests
            .iter()
            .filter(|request| fresh && !request.completed)
            .map(|request| Running {
                id: request.id.clone(),
                prompt: request.prompt,
                // Runtimes report prefill progress only while the prompt is read.
                phase: match request.prefilled {
                    _ if request.queued => Phase::Queued,
                    Some(done) => Phase::Prefill(done.min(request.prompt)),
                    None => Phase::Generate,
                },
                output: request.output,
                tps: request.output_tps,
            })
            .collect();
        // The work under way leads, so a short panel never hides the prefill
        // that is holding up every other request.
        running.sort_by_key(|request| match request.phase {
            Phase::Prefill(_) => 0,
            Phase::Generate => 1,
            Phase::Queued => 2,
        });
        if self.speeds_from.as_ref() != Some(&sample.llm_provider) {
            self.speeds_from = running
                .iter()
                .any(|request| request.tps.is_some())
                .then(|| sample.llm_provider.clone());
        }
        self.running = if self.speeds_from.is_some() {
            running
        } else {
            Vec::new()
        };
        for request in &sample.llm_requests {
            let key = format!("{}\0{}\0{}", request.provider, request.model, request.id);
            if let Some(old) = self.timings.iter_mut().find(|old| old.0 == key) {
                if let Some(ttft) = request.ttft_ms {
                    old.1 = Some(ttft);
                    old.2 = request.observed_at.unwrap_or(old.2);
                }
            } else {
                self.timings.push_back((
                    key,
                    request.ttft_ms,
                    request.observed_at.unwrap_or_else(SystemTime::now),
                ));
            }
        }
        while self.timings.len() > 240 {
            self.timings.pop_front();
        }
    }

    pub fn has_latency(&self) -> bool {
        self.timings.iter().any(|p| p.1.is_some())
    }
}
