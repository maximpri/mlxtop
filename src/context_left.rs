// SPDX-License-Identifier: MIT
//! How much more context fits: the tokens each request in flight can still
//! add before the memory its KV cache grows into runs out, or before its
//! context window does. It is a forecast, not an impact: plain text that
//! colours nothing, and nothing at all while an input is unknown.
//!
//! The room comes from the runtime where it reports some: the KV cache it
//! has left in tokens, or what its own memory plan has left in bytes. A
//! runtime that sizes its KV cache for its context window when it loads a
//! model (llama.cpp with `-c`) has that window whatever memory is left, so
//! what is left there is the window less the context held. A runtime that
//! grows its KV cache as tokens arrive (MLX) has the machine's room, as the
//! memory panel gives it: free memory and file cache, or the room under the
//! GPU limit where that is less, less a margin.
use crate::domain::{RequestUsage, Sample};
use crate::formatting::{bytes, compact_tokens};
use crate::gpu::Memory;
use crate::memory_headroom::Left;

/// Kept back from the machine's memory: the system and the runtime's own
/// working memory draw on it as well.
pub(crate) const MARGIN: u64 = 1 << 30;

/// Runtimes that size the KV cache for their context window when they load
/// a model: llama.cpp's `-c`, and the runtimes built on it.
const SIZED_AT_LOAD: [&str; 4] = ["llama.cpp", "KoboldCpp", "Jan", "Ollama"];

/// Runtimes that grow the KV cache in memory as tokens arrive (MLX).
const GROWING: [&str; 3] = ["oMLX", "mlx-lm", "mlx-serve"];

/// Where the room for more context comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Room {
    /// KV cache the runtime reports left, in tokens, shared by its requests.
    Tokens(u64),
    /// Bytes the runtime's own memory plan has left for the KV cache.
    Plan(u64),
    /// Memory the machine has left for the KV cache to grow into, and what
    /// that memory is.
    Machine { bytes: u64, what: &'static str },
    /// The KV cache was sized for the window when the model loaded.
    Window,
}

/// What the forecast is worked out from. `None` is unknown, never zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Inputs {
    pub(crate) room: Room,
    /// KV bytes one more token of context costs.
    pub(crate) kv_per_token: Option<u64>,
    /// What a request holds whatever its length.
    pub(crate) state: Option<u64>,
    /// Requests the forecast is for: those in flight, or the next one.
    pub(crate) requests: u64,
    /// Of them, those that do not hold their state yet.
    pub(crate) new: u64,
    /// The context a request may reach, and the most one in flight holds.
    pub(crate) window: u64,
    pub(crate) held: u64,
    /// Kept back from the room.
    pub(crate) margin: u64,
}

/// Tokens each request can still add, and the two limits it is the
/// smaller of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Forecast {
    pub(crate) tokens: u64,
    pub(crate) requests: u64,
    /// What memory alone leaves each request: `None` where only the
    /// window limits it.
    pub(crate) by_memory: Option<u64>,
    /// What the window alone leaves: the window less the context held.
    pub(crate) by_window: u64,
}

/// Tokens left per request = (room − margin − state × requests without
/// theirs yet) ÷ (KV bytes per token × requests), capped at the window less
/// the context already held. A request in flight already holds its state:
/// the room was measured after it, so it is not taken off again. `None`
/// when an input the room needs is unknown.
pub(crate) fn forecast(inputs: &Inputs) -> Option<Forecast> {
    let requests = inputs.requests.max(1);
    let by_window = inputs.window.saturating_sub(inputs.held);
    let by_memory = match inputs.room {
        Room::Window => None,
        Room::Tokens(tokens) => Some(tokens / requests),
        Room::Plan(room) | Room::Machine { bytes: room, .. } => {
            let kv = inputs.kv_per_token?;
            // A runtime's own plan keeps each request's state apart from its
            // KV room (Splash plans them separately); only memory the
            // machine has left must also hold the next request's state.
            let state = if inputs.new == 0 || matches!(inputs.room, Room::Plan(_)) {
                0
            } else {
                inputs.state?.saturating_mul(inputs.new)
            };
            let free = room.saturating_sub(inputs.margin);
            match (free.checked_sub(state), kv) {
                (None, _) => Some(0),
                // Without KV layers (a pure state-space model) memory does
                // not limit how far a request grows once its state fits.
                (Some(_), 0) => None,
                (Some(free), kv) => Some(
                    u64::try_from(u128::from(free) / (u128::from(kv) * u128::from(requests)))
                        .unwrap_or(u64::MAX),
                ),
            }
        }
    };
    Some(Forecast {
        tokens: by_memory.map_or(by_window, |memory| memory.min(by_window)),
        requests,
        by_memory,
        by_window,
    })
}

/// The requests in flight: how many, the most context one holds, and the
/// context they hold together.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct InFlight {
    pub(crate) requests: u64,
    pub(crate) largest: u64,
    pub(crate) total: u64,
}

/// The requests in flight, from the runtime's list (a request writing or
/// reading its prompt, not one waiting), else its count with the context it
/// reports. `None` while requests run whose context is not known.
pub(crate) fn in_flight(sample: &Sample) -> Option<InFlight> {
    let context =
        |request: &&RequestUsage| request.prompt.saturating_add(request.output.unwrap_or(0));
    let running: Vec<&RequestUsage> = sample
        .llm_requests
        .iter()
        .filter(|request| !request.completed && !request.queued)
        .collect();
    if !running.is_empty() {
        return Some(InFlight {
            requests: running.len() as u64,
            largest: running.iter().map(context).max().unwrap_or(0),
            total: running.iter().map(context).fold(0, u64::saturating_add),
        });
    }
    match sample.llm_active_requests.unwrap_or(0) {
        0 => Some(InFlight::default()),
        1 => crate::analysis::llm_context_tokens(sample).map(|context| InFlight {
            requests: 1,
            largest: context,
            total: context,
        }),
        _ => None,
    }
}

/// Whether Splash runs the model: on its own, or as oMLX's engine.
fn splash(sample: &Sample) -> bool {
    sample.llm_engine.as_deref() == Some("Splash") || sample.llm_provider == "Splash"
}

/// What one request in flight holds whatever its length. Splash keeps two
/// state cells for a request it decodes, beside its draft's attention
/// cache; the model's facts give one cell with the draft's context window,
/// so a Splash request holds at least twice that. (For Qwen3.8-27B with its
/// DFlash2 draft that is 305 MiB of the about 340 MiB a request holds: the
/// draft's 40 MiB attention cache is not among the facts.)
pub(crate) fn request_state(sample: &Sample) -> Option<u64> {
    let state = sample.model.fixed_state_bytes?;
    Some(if splash(sample) {
        state.saturating_mul(2)
    } else {
        state
    })
}

/// The room the machine has for a growing KV cache: on one pool, what the
/// memory panel says is left; on cards with memory of their own, the VRAM
/// left.
fn machine_room(sample: &Sample, memory: Memory) -> Option<Room> {
    let (bytes, what) = match memory {
        Memory::Dedicated { used, total } => (total.saturating_sub(used), "of VRAM left"),
        _ => match crate::memory_headroom::left_for(sample, memory)? {
            Left::Ram(bytes) => (bytes, "of free memory and file cache"),
            Left::ToLimit(bytes) => (bytes, "to the GPU limit"),
            Left::OverLimit => (0, "to the GPU limit"),
        },
    };
    Some(Room::Machine { bytes, what })
}

/// The forecast's inputs, read from `sample` on a machine whose GPU memory
/// is `memory`. `None` when the runtime, its window, the requests in flight
/// or where the room comes from is not known.
pub(crate) fn inputs_for(sample: &Sample, memory: Memory) -> Option<Inputs> {
    if sample.llm_status == "stale" {
        return None;
    }
    let facts = &sample.model;
    let window = facts
        .context_configured
        .or(facts.context_trained)
        .filter(|window| *window > 0)?;
    let flight = in_flight(sample)?;
    let provider = sample.llm_provider.as_str();
    // A runtime that sizes its KV cache per request when it loads the model
    // has its window left whatever it reports: llama.cpp's KV room is per
    // slot, not a pool its requests share.
    let room = if SIZED_AT_LOAD.contains(&provider) {
        Room::Window
    } else if let Some(tokens) = sample.llm_kv_free_tokens {
        Room::Tokens(tokens)
    } else if let Some(bytes) = sample.llm_kv_free_bytes {
        Room::Plan(bytes)
    } else if GROWING.contains(&provider) && !splash(sample) && !sample.llm_remote {
        machine_room(sample, memory)?
    } else {
        return None;
    };
    Some(Inputs {
        room,
        kv_per_token: facts.kv_bytes_per_token(),
        state: request_state(sample),
        requests: flight.requests.max(1),
        new: u64::from(flight.requests == 0),
        window,
        held: flight.largest,
        margin: if matches!(room, Room::Machine { .. }) {
            MARGIN
        } else {
            0
        },
    })
}

/// The forecast on a machine whose GPU memory is `memory`.
pub(crate) fn forecast_for(sample: &Sample, memory: Memory) -> Option<Forecast> {
    forecast(&inputs_for(sample, memory)?)
}

/// An estimate of tokens, rounded down: `≈129k tokens`, `≈1.2M tokens`,
/// `≈850 tokens`, with `more` before the noun where it is asked for.
fn approx(tokens: u64, more: bool) -> String {
    let count = if tokens >= 1_000_000 {
        format!("{:.1}M", (tokens / 100_000) as f64 / 10.0)
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    };
    let more = if more { "more " } else { "" };
    let noun = if tokens == 1 { "token" } else { "tokens" };
    format!("≈{count} {more}{noun}")
}

fn plural(count: u64, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

impl Forecast {
    /// The memory panel's wording: `≈37k tokens at 2 requests`.
    pub(crate) fn label(&self) -> String {
        format!(
            "{} at {}",
            approx(self.tokens, false),
            plural(self.requests, "request")
        )
    }
}

/// The forecast's arithmetic for Diagnostics: what is left and for which
/// requests, what memory and the window each allow, and what the requests
/// in flight hold. Nothing while an input is unknown.
pub(crate) fn lines(sample: &Sample) -> Vec<String> {
    inputs_for(sample, sample.gpu_memory())
        .and_then(|inputs| Some(arithmetic(sample, &inputs, &forecast(&inputs)?)))
        .unwrap_or_default()
}

fn arithmetic(sample: &Sample, inputs: &Inputs, forecast: &Forecast) -> Vec<String> {
    let flight = in_flight(sample).unwrap_or_default();
    let (next, more) = (
        approx(forecast.tokens, false),
        approx(forecast.tokens, true),
    );
    let mut lines = vec![match flight.requests {
        0 => format!("Context left: {next} for the next request (estimate)"),
        1 => format!("Context left: {more} for the request in flight (estimate)"),
        n => format!("Context left: {more} for each of the {n} requests in flight (estimate)"),
    }];
    let engine = if splash(sample) {
        "Splash"
    } else {
        sample.llm_provider.as_str()
    };
    let each = if forecast.requests > 1 { " each" } else { "" };
    // The forecast is the smaller limit (memory's, where they tie); with
    // only one limit, nothing to say.
    let smaller = |smaller: bool| if smaller { ", the smaller" } else { "" };
    let by_memory_smaller = forecast
        .by_memory
        .is_some_and(|memory| memory <= forecast.by_window);
    let shared_out = |per_token: String| {
        if forecast.requests > 1 {
            format!("({} × {per_token})", plural(forecast.requests, "request"))
        } else {
            per_token
        }
    };
    // What memory leaves, as the runtime or the machine gives the room: a
    // room in bytes is shared out at the KV a token costs, after the margin
    // and the state of a request not yet running.
    let state = inputs
        .state
        .filter(|state| inputs.new > 0 && *state > 0 && !matches!(inputs.room, Room::Plan(_)))
        .map(|state| format!(" − {} of state for it", bytes(state)))
        .unwrap_or_default();
    let per_token = inputs
        .kv_per_token
        .map(|kv| shared_out(format!("{} a token", bytes(kv))))
        .unwrap_or_default();
    let room = match inputs.room {
        Room::Tokens(free) => Some(format!(
            "{} of KV cache {engine} reports free ÷ {}",
            tokens(free),
            plural(forecast.requests, "request")
        )),
        Room::Plan(room) if state.is_empty() => Some(format!(
            "{} left in {engine}'s memory plan ÷ {per_token}",
            bytes(room)
        )),
        Room::Plan(room) => Some(format!(
            "({} left in {engine}'s memory plan{state}) ÷ {per_token}",
            bytes(room)
        )),
        Room::Machine { bytes: room, what } => Some(format!(
            "({} {what} − {} kept back{state}) ÷ {per_token}",
            bytes(room),
            bytes(inputs.margin)
        )),
        Room::Window => None,
    };
    if let (Some(room), Some(by_memory)) = (room, forecast.by_memory) {
        lines.push(format!(
            "By memory: {room} = {}{each}{}",
            tokens(by_memory),
            smaller(by_memory_smaller)
        ));
    }
    let window = compact_tokens(inputs.window);
    let held = compact_tokens(inputs.held);
    let window_smaller = smaller(forecast.by_memory.is_some() && !by_memory_smaller);
    let rest = if inputs.held == 0 {
        window_smaller.to_owned()
    } else {
        format!(
            " − {held} held = {}{each}{window_smaller}",
            tokens(forecast.by_window)
        )
    };
    lines.push(match inputs.room {
        Room::Window => format!(
            "By window: {engine} sized its KV cache for {window} tokens a request when it loaded the model{rest}"
        ),
        _ => format!("By window: {window} tokens allowed{rest}"),
    });
    // What the requests in flight hold, and in a memory plan the share of
    // the plan's room for requests that is.
    if let (true, Some(kv)) = (flight.requests > 0, inputs.kv_per_token) {
        let state = inputs.state.unwrap_or(0);
        let mut line = format!(
            "Held: {} in {}: {} of KV cache",
            tokens(flight.total),
            plural(flight.requests, "request"),
            bytes(flight.total.saturating_mul(kv))
        );
        if state > 0 {
            line.push_str(&format!(
                " and ≈{} of state",
                bytes(flight.requests.saturating_mul(state))
            ));
            if flight.requests > 1 {
                line.push_str(&format!(" ({} each)", bytes(state)));
            }
        }
        // The plan's room is KV cache only: the state is planned apart.
        if let (Room::Plan(left), true) = (inputs.room, kv > 0) {
            let kv_held = flight.total.saturating_mul(kv);
            let room = kv_held.saturating_add(left);
            line.push_str(&format!(
                ", {}% of the KV cache {engine}'s plan has ({})",
                share(kv_held, room),
                bytes(room)
            ));
        }
        lines.push(line);
    }
    lines
}

/// A count of tokens: `1 token`, `76.4k tokens`.
fn tokens(count: u64) -> String {
    if count == 1 {
        "1 token".into()
    } else {
        format!("{} tokens", compact_tokens(count))
    }
}

/// `part` of `whole` in whole percent, rounded down so a plan nearly full
/// never reads as full.
pub(crate) fn share(part: u64, whole: u64) -> u64 {
    if whole == 0 {
        return 0;
    }
    u64::try_from(u128::from(part) * 100 / u128::from(whole)).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[path = "tests/context_left.rs"]
mod tests;
