// SPDX-License-Identifier: MIT
//! A request's true decode speed and cached prefix, from what the runtime
//! reports at each poll.
//!
//! oMLX reports `tokens_per_second` as tokens over time since generation
//! began. That average hides a stall: with no new tokens it decays slowly
//! instead of reading zero. Tokens produced between two polls over the time
//! between them is the speed the operator sees.
//!
//! A prefill starts with `processed` at the cached prefix and counts the new
//! tokens from there, so the first value seen is the request's cache reuse.
use crate::domain::RequestUsage;
use std::collections::HashMap;
use std::time::Instant;

#[derive(Default)]
pub(crate) struct DecodeTracker {
    /// Output tokens and when they were read, per generating request.
    decoded: HashMap<String, (u64, Instant)>,
    /// The `processed` count first seen for each prefilling request.
    prefix: HashMap<String, u64>,
}

impl DecodeTracker {
    /// Replace each generating request's average with its speed since the
    /// last poll, and give each prefilling request its cached prefix. Returns
    /// the summed speed of the generating requests, when every one has one.
    pub(crate) fn apply(&mut self, requests: &mut [RequestUsage], now: Instant) -> Option<f64> {
        let mut decoded = HashMap::new();
        let mut prefix = HashMap::new();
        let mut total = Some(0.0);
        let mut generating = 0;
        for request in requests.iter_mut().filter(|r| !r.completed && !r.queued) {
            if let Some(done) = request.prefilled {
                let start = *self.prefix.get(&request.id).unwrap_or(&done);
                prefix.insert(request.id.clone(), start);
                if request.cached.is_none() && start > 0 && start < request.prompt {
                    request.cached = Some(start);
                }
                continue;
            }
            let Some(tokens) = request.output else {
                continue;
            };
            generating += 1;
            if let Some((before, at)) = self.decoded.get(&request.id) {
                let seconds = now.saturating_duration_since(*at).as_secs_f64();
                if seconds >= 0.2 && tokens >= *before {
                    request.output_tps = Some((tokens - before) as f64 / seconds);
                }
            }
            // The first poll of a request keeps the runtime's own figure.
            total = total.zip(request.output_tps).map(|(sum, tps)| sum + tps);
            decoded.insert(request.id.clone(), (tokens, now));
        }
        self.decoded = decoded;
        self.prefix = prefix;
        total.filter(|_| generating > 0)
    }
}

#[cfg(test)]
#[path = "tests/decode_tracker.rs"]
mod tests;
