// SPDX-License-Identifier: MIT
use super::*;
use crate::domain::TelemetrySource;
use crate::model_facts::ModelFacts;

/// What `requests` requests holding `tokens` tokens of context between them
/// hold in memory: their KV cache and each one's state.
fn held_bytes(tokens: u64, requests: u64, kv_per_token: u64, state: u64) -> u64 {
    tokens
        .saturating_mul(kv_per_token)
        .saturating_add(requests.saturating_mul(state))
}

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// The owner's Mac Studio: Splash's memory plan (the `serve-native`
/// argument) less the weights it loads (vision included) and the bf16
/// DFlash2 draft leaves this for requests.
const PLAN: u64 = 25_837_922_488;
const WEIGHTS: u64 = 16_971_398_624;
const DRAFT: u64 = 3_848_808_960;
const REMAINDER: u64 = PLAN - WEIGHTS - DRAFT;
/// Int8 KV for Qwen3.8-27B-oQ4e-mtp's 16 full-attention layers.
const KV: u64 = 33_280;
/// What a Splash request in flight holds whatever its length, as WP3
/// counted it: two state cells, the draft's attention cache and its window.
const ACTIVE_STATE: u64 = 2 * 154_140_672 + 41_943_040 + 5_898_240;
/// The facts' state: one cell with the draft's window.
const FACTS_STATE: u64 = 154_140_672 + 5_898_240;
/// Median and 90th-percentile contexts of the Splash month.
const MEDIAN: u64 = 1_932;
const P90: u64 = 63_673;
const WINDOW: u64 = 131_072;

fn plan(left: u64, requests: u64, held: u64) -> Inputs {
    Inputs {
        room: Room::Plan(left),
        kv_per_token: Some(KV),
        state: Some(ACTIVE_STATE),
        requests: requests.max(1),
        new: u64::from(requests == 0),
        window: WINDOW,
        held,
        margin: 0,
    }
}

#[test]
fn the_owners_plan_fits_one_long_context_and_two_fill_it() {
    assert_eq!(REMAINDER, 5_017_714_904);
    assert_eq!(ACTIVE_STATE / MIB, 339);
    // One p90 context: its KV alone is 42% of the plan's remainder, 49%
    // with the state a Splash request holds.
    let one = held_bytes(P90, 1, KV, ACTIVE_STATE);
    assert_eq!(P90 * KV, 2_119_037_440);
    assert_eq!(share(P90 * KV, REMAINDER), 42);
    assert_eq!(one, 2_475_160_064);
    assert_eq!(share(one, REMAINDER), 49);
    // Two: 84% as KV alone, 90% with the facts' 152.6 MiB state each, and
    // 98% (98.66%) with what a request in flight really holds.
    let two = held_bytes(2 * P90, 2, KV, ACTIVE_STATE);
    assert_eq!(share(2 * P90 * KV, REMAINDER), 84);
    assert_eq!(
        share(held_bytes(2 * P90, 2, KV, FACTS_STATE), REMAINDER),
        90
    );
    assert_eq!(two, 4_950_320_128);
    assert_eq!(share(two, REMAINDER), 98);

    // What is left in the plan then, and what that leaves each request.
    let alone = forecast(&plan(REMAINDER - one, 1, P90)).unwrap();
    assert_eq!(alone.by_memory, Some(76_398));
    assert_eq!(alone.by_window, WINDOW - P90);
    assert_eq!(alone.tokens, 67_399, "the window is nearer than memory");
    assert_eq!(alone.label(), "≈67k tokens at 1 request");
    let pair = forecast(&plan(REMAINDER - two, 2, P90)).unwrap();
    assert_eq!(REMAINDER - two, 67_394_776);
    assert_eq!(pair.tokens, 1_012, "64.3 MiB ÷ (2 × 32.5 KiB)");
    assert_eq!(pair.label(), "≈1k tokens at 2 requests");
}

#[test]
fn the_median_request_reads_nearly_all_its_window_left() {
    let held = held_bytes(MEDIAN, 1, KV, ACTIVE_STATE);
    let median = forecast(&plan(REMAINDER - held, 1, MEDIAN)).unwrap();
    assert_eq!(median.by_memory, Some(138_139));
    assert_eq!(median.tokens, WINDOW - MEDIAN);
    // 98.5% of the window: the window, not memory, is what limits it.
    assert_eq!(share(median.tokens, WINDOW), 98);
    assert_eq!(median.label(), "≈129k tokens at 1 request");
    // With nothing in flight, the next request's state comes from the
    // plan's state budget, not its KV room, and it has its whole window.
    let next = forecast(&plan(REMAINDER, 0, 0)).unwrap();
    assert_eq!(next.by_memory, Some(REMAINDER / KV));
    assert_eq!(next.tokens, WINDOW);
    assert_eq!(next.label(), "≈131k tokens at 1 request");
}

#[test]
fn the_arithmetic_follows_the_formula_for_every_room() {
    let inputs = |room| Inputs {
        room,
        kv_per_token: Some(100),
        state: Some(1_000),
        requests: 2,
        new: 0,
        window: 10_000,
        held: 4_000,
        margin: 0,
    };
    // Tokens the runtime reports free are shared by its requests.
    let tokens = forecast(&inputs(Room::Tokens(9_000))).unwrap();
    assert_eq!((tokens.by_memory, tokens.tokens), (Some(4_500), 4_500));
    // Bytes: (room − margin − state of requests not yet holding theirs)
    // ÷ (KV per token × requests), then the window less what is held.
    let mut machine = inputs(Room::Machine {
        bytes: 2_000_000,
        what: "to the GPU limit",
    });
    machine.margin = 500_000;
    assert_eq!(forecast(&machine).unwrap().by_memory, Some(7_500));
    machine.new = 1;
    assert_eq!(forecast(&machine).unwrap().by_memory, Some(7_495));
    assert_eq!(forecast(&machine).unwrap().tokens, 6_000);
    // A margin larger than the room leaves nothing, never a wrap-around.
    machine.margin = 3_000_000;
    assert_eq!(forecast(&machine).unwrap().tokens, 0);
    // Machine memory the next request's state does not fit in: nothing.
    let mut full = inputs(Room::Machine {
        bytes: 500,
        what: "to the GPU limit",
    });
    full.new = 1;
    assert_eq!(forecast(&full).unwrap().by_memory, Some(0));
    // A runtime's plan keeps state apart: its room is KV cache alone.
    let mut kv_only = inputs(Room::Plan(500));
    kv_only.new = 1;
    assert_eq!(forecast(&kv_only).unwrap().by_memory, Some(2));
    // Sized at load: the window is all that limits it.
    let window = forecast(&inputs(Room::Window)).unwrap();
    assert_eq!((window.by_memory, window.tokens), (None, 6_000));
    // A context past its window has nothing left, not a negative.
    let mut past = inputs(Room::Window);
    past.held = 12_000;
    assert_eq!(forecast(&past).unwrap().tokens, 0);
    // No KV layers (a state-space model): memory limits only the state.
    let mut flat = inputs(Room::Plan(2_000));
    flat.kv_per_token = Some(0);
    assert_eq!(forecast(&flat).unwrap().by_memory, None);
    let mut flat = inputs(Room::Machine {
        bytes: 2_000,
        what: "to the GPU limit",
    });
    flat.kv_per_token = Some(0);
    flat.new = 1;
    flat.state = Some(5_000);
    assert_eq!(forecast(&flat).unwrap().by_memory, Some(0));
    // Requests in flight never count as fewer than one.
    let mut none = inputs(Room::Tokens(900));
    none.requests = 0;
    assert_eq!(forecast(&none).unwrap().requests, 1);
}

#[test]
fn unknown_inputs_give_no_forecast() {
    let mut inputs = plan(REMAINDER, 0, 0);
    inputs.kv_per_token = None;
    assert_eq!(forecast(&inputs), None);
    // The state matters only to a request that does not hold it yet, and
    // only in memory the machine has left: a runtime's plan keeps it apart.
    let mut inputs = plan(REMAINDER, 0, 0);
    inputs.state = None;
    assert!(forecast(&inputs).is_some());
    inputs.room = Room::Machine {
        bytes: REMAINDER,
        what: "to the GPU limit",
    };
    assert_eq!(forecast(&inputs), None);
    inputs.new = 0;
    assert!(forecast(&inputs).is_some());
    // Tokens and windows need neither.
    inputs.room = Room::Tokens(10);
    inputs.kv_per_token = None;
    assert!(forecast(&inputs).is_some());
}

#[test]
fn estimates_round_down_and_say_how_many_requests() {
    let label = |tokens, requests| {
        Forecast {
            tokens,
            requests,
            by_memory: None,
            by_window: tokens,
        }
        .label()
    };
    assert_eq!(label(0, 1), "≈0 tokens at 1 request");
    assert_eq!(label(999, 1), "≈999 tokens at 1 request");
    assert_eq!(label(37_999, 2), "≈37k tokens at 2 requests");
    assert_eq!(label(1_299_999, 3), "≈1.2M tokens at 3 requests");
    assert_eq!(share(1, 0), 0);
    assert_eq!(share(999, 1_000), 99, "nearly full never reads as full");
}

/// The owner's model as oMLX serves it through Splash, with what Splash's
/// plan has left after the requests in flight.
fn owner(left: Option<u64>, contexts: &[u64]) -> Sample {
    Sample {
        llm_provider: "oMLX".into(),
        llm_engine: Some("Splash".into()),
        llm_model: "Qwen3.8-27B-oQ4e-mtp".into(),
        llm_status: "generating".into(),
        llm_source: TelemetrySource::Live,
        llm_kv_free_bytes: left,
        llm_active_requests: Some(contexts.len() as u64),
        llm_requests: contexts
            .iter()
            .enumerate()
            .map(|(index, context)| RequestUsage {
                id: format!("r{index}"),
                prompt: context - 500,
                output: Some(500),
                ..RequestUsage::default()
            })
            .collect(),
        model: ModelFacts {
            kv_layers: Some(16),
            kv_heads: Some(4),
            head_dim: Some(256),
            kv_dtype: Some("int8".into()),
            kv_bytes_per_element: Some(1.0 + 1.0 / 64.0),
            fixed_state_bytes: Some(FACTS_STATE),
            context_configured: Some(WINDOW),
            context_trained: Some(262_144),
            draft: Some(true),
            ..ModelFacts::default()
        },
        ..Sample::default()
    }
}

#[test]
fn a_sample_from_the_owners_server_gives_the_same_forecast() {
    let one = owner(Some(REMAINDER - P90 * KV - ACTIVE_STATE), &[P90]);
    assert_eq!(one.model.kv_bytes_per_token(), Some(KV));
    let inputs = inputs_for(&one, Memory::Shared).unwrap();
    assert_eq!(inputs.room, Room::Plan(2_542_554_840));
    assert_eq!((inputs.requests, inputs.new, inputs.held), (1, 0, P90));
    assert_eq!(inputs.margin, 0, "a runtime's own plan is taken as it is");
    let forecast = forecast_for(&one, Memory::Shared).unwrap();
    assert_eq!(forecast.label(), "≈67k tokens at 1 request");
    // Splash holds two state cells for a request in flight: at least twice
    // the facts' state, 305 MiB of WP3's 340.
    assert_eq!(request_state(&one), Some(2 * FACTS_STATE));
    let two = owner(Some(REMAINDER - 2 * (P90 * KV + ACTIVE_STATE)), &[P90, P90]);
    assert_eq!(
        forecast_for(&two, Memory::Shared).unwrap().label(),
        "≈1k tokens at 2 requests"
    );
    assert_eq!(
        lines(&two),
        [
            "Context left: ≈1k more tokens for each of the 2 requests in flight (estimate)",
            "By memory: 64.3 MiB left in Splash's memory plan ÷ (2 requests × 32.5 KiB a token) = 1.0k tokens each, the smaller",
            "By window: 131.1k tokens allowed − 63.7k held = 67.4k tokens each",
            "Held: 127.3k tokens in 2 requests: 3.9 GiB of KV cache and ≈610.5 MiB of state (305.2 MiB each), 98% of the KV cache Splash's plan has (4.0 GiB)",
        ]
    );
    let median = owner(Some(REMAINDER - MEDIAN * KV - ACTIVE_STATE), &[MEDIAN]);
    assert_eq!(
        forecast_for(&median, Memory::Shared).unwrap().label(),
        "≈129k tokens at 1 request"
    );
    // Idle, the next request has its whole window.
    let idle = owner(Some(REMAINDER), &[]);
    assert_eq!(
        lines(&idle)[..3],
        [
            "Context left: ≈131k tokens for the next request (estimate)",
            "By memory: 4.7 GiB left in Splash's memory plan ÷ 32.5 KiB a token = 150.8k tokens",
            "By window: 131.1k tokens allowed, the smaller",
        ]
    );
    assert_eq!(lines(&idle).len(), 2 + 1, "nothing held, so no Held line");
}

#[test]
fn splash_without_its_plan_says_nothing() {
    // Splash plans its memory: what the machine has left is not its room.
    let blind = owner(None, &[P90]);
    assert_eq!(inputs_for(&blind, Memory::Shared), None);
    assert!(lines(&blind).is_empty());
    // A stale reading, or a window nobody gave, says nothing either.
    let mut stale = owner(Some(REMAINDER), &[]);
    stale.llm_status = "stale".into();
    assert_eq!(forecast_for(&stale, Memory::Shared), None);
    let mut windowless = owner(Some(REMAINDER), &[]);
    windowless.model.context_configured = None;
    windowless.model.context_trained = None;
    assert_eq!(forecast_for(&windowless, Memory::Shared), None);
    // The trained context stands in for a window the runtime did not give.
    windowless.model.context_trained = Some(40_000);
    assert_eq!(
        forecast_for(&windowless, Memory::Shared).unwrap().tokens,
        40_000
    );
    // KV per token unknown: a plan in bytes cannot be shared out.
    let mut unknown_kv = owner(Some(REMAINDER), &[]);
    unknown_kv.model.kv_bytes_per_element = None;
    assert_eq!(forecast_for(&unknown_kv, Memory::Shared), None);
}

/// oMLX's own engine, which grows the KV cache in the Mac's memory.
fn growing(contexts: &[u64]) -> Sample {
    let mut sample = owner(None, contexts);
    sample.llm_engine = None;
    sample.model.fixed_state_bytes = Some(153_944_064);
    sample.vm_available = true;
    sample.total_memory = 36 * GIB;
    sample.wired = 26 * GIB;
    sample.anonymous = 2 * GIB;
    sample.file_backed = 3 * GIB;
    sample.gpu_in_use = Some(25 * GIB);
    sample.metal.resource_limit = Some(32 * GIB);
    // A Mac's sample names its GPU, so it reads as one pool on any OS.
    sample.metal.device_name = Some("Apple M4 Max".into());
    sample.metal.gpu_cores = Some(32);
    sample
}

#[test]
fn a_growing_cache_has_the_room_the_memory_panel_gives() {
    // 8 GiB of free memory and cache, 7 GiB to the GPU limit: the limit.
    let one = growing(&[P90]);
    let inputs = inputs_for(&one, Memory::Shared).unwrap();
    assert_eq!(
        inputs.room,
        Room::Machine {
            bytes: 7 * GIB,
            what: "to the GPU limit"
        }
    );
    assert_eq!((inputs.margin, inputs.new), (MARGIN, 0));
    let forecast = forecast(&inputs).unwrap();
    assert_eq!(forecast.by_memory, Some(6 * GIB / KV));
    assert_eq!(forecast.tokens, WINDOW - P90);
    assert_eq!(
        lines(&one)[1],
        "By memory: (7.0 GiB to the GPU limit − 1.0 GiB kept back) ÷ 32.5 KiB a token = 193.6k tokens"
    );
    // A third request in flight shares that room three ways.
    let three = growing(&[P90, 20_000, 10_000]);
    let forecast = forecast_for(&three, Memory::Shared).unwrap();
    assert_eq!(
        (forecast.requests, forecast.tokens),
        (3, 6 * GIB / (3 * KV))
    );
    // Free memory and cache nearer than the limit, and past the limit.
    let mut tight = growing(&[]);
    tight.metal.resource_limit = Some(34 * GIB);
    assert!(matches!(
        inputs_for(&tight, Memory::Shared).unwrap().room,
        Room::Machine {
            bytes,
            what: "of free memory and file cache"
        } if bytes == 8 * GIB
    ));
    tight.gpu_in_use = Some(35 * GIB);
    let over = forecast_for(&tight, Memory::Shared).unwrap();
    assert_eq!((over.by_memory, over.tokens), (Some(0), 0));
    assert_eq!(over.label(), "≈0 tokens at 1 request");
    // Cards with memory of their own: the VRAM they have left.
    let vram = inputs_for(
        &growing(&[]),
        Memory::Dedicated {
            used: 20 * GIB,
            total: 24 * GIB,
        },
    )
    .unwrap();
    assert_eq!(
        vram.room,
        Room::Machine {
            bytes: 4 * GIB,
            what: "of VRAM left"
        }
    );
    // A remote runtime's memory is not this machine's.
    let mut remote = growing(&[]);
    remote.llm_remote = true;
    assert_eq!(inputs_for(&remote, Memory::Shared), None);
    // Without the OS's composition there is no room to read.
    let mut unread = growing(&[]);
    unread.vm_available = false;
    assert_eq!(inputs_for(&unread, Memory::Shared), None);
}

#[test]
fn runtimes_that_size_the_cache_at_load_have_their_window_left() {
    let mut llama = growing(&[12_100]);
    llama.llm_provider = "llama.cpp".into();
    llama.model.context_configured = Some(32_768);
    let forecast = forecast_for(&llama, Memory::Shared).unwrap();
    assert_eq!((forecast.by_memory, forecast.tokens), (None, 20_668));
    assert_eq!(
        lines(&llama)[1..],
        [
            "By window: llama.cpp sized its KV cache for 32.8k tokens a request when it loaded the model − 12.1k held = 20.7k tokens",
            "Held: 12.1k tokens in 1 request: 384.0 MiB of KV cache and ≈146.8 MiB of state",
        ]
    );
    llama.llm_requests.clear();
    llama.llm_active_requests = Some(0);
    assert_eq!(
        lines(&llama)[1],
        "By window: llama.cpp sized its KV cache for 32.8k tokens a request when it loaded the model"
    );
    // A runtime that reports its free KV cache in tokens is taken at its word.
    let mut vllm = growing(&[10_000, 10_000]);
    vllm.llm_provider = "vLLM".into();
    vllm.llm_kv_free_tokens = Some(74_200);
    vllm.model.context_configured = Some(32_768);
    let forecast = forecast_for(&vllm, Memory::Shared).unwrap();
    assert_eq!(
        (forecast.by_memory, forecast.tokens),
        (Some(37_100), 22_768)
    );
    assert_eq!(
        lines(&vllm)[1],
        "By memory: 74.2k tokens of KV cache vLLM reports free ÷ 2 requests = 37.1k tokens each"
    );
    // A runtime whose cache mlxtop cannot place says nothing.
    let mut unknown = growing(&[]);
    unknown.llm_provider = "LM Studio".into();
    assert_eq!(inputs_for(&unknown, Memory::Shared), None);
}

#[test]
fn requests_in_flight_come_from_the_runtimes_list_or_its_count() {
    let mut sample = growing(&[30_000, 10_000]);
    sample.llm_requests.push(RequestUsage {
        prompt: 90_000,
        queued: true,
        ..RequestUsage::default()
    });
    sample.llm_requests.push(RequestUsage {
        prompt: 90_000,
        completed: true,
        ..RequestUsage::default()
    });
    assert_eq!(
        in_flight(&sample),
        Some(InFlight {
            requests: 2,
            largest: 30_000,
            total: 40_000
        })
    );
    // No list: one request with the context the runtime reports.
    sample.llm_requests.clear();
    sample.llm_active_requests = Some(1);
    sample.llm_prompt_tokens = Some(7_000);
    sample.llm_output_tokens = Some(100);
    assert_eq!(in_flight(&sample).unwrap().largest, 7_100);
    // Several without a list: their contexts are unknown, so is the rest.
    sample.llm_active_requests = Some(2);
    assert_eq!(in_flight(&sample), None);
    assert_eq!(forecast_for(&sample, Memory::Shared), None);
    sample.llm_active_requests = None;
    assert_eq!(in_flight(&sample), Some(InFlight::default()));
    // oMLX's own engine holds one state per request.
    assert_eq!(request_state(&sample), Some(153_944_064));
    sample.model.fixed_state_bytes = None;
    assert_eq!(request_state(&sample), None);
}

#[test]
fn the_arithmetic_names_one_limit_as_the_smaller() {
    // A tie goes to memory: one limit is named, never both.
    let mut tie = growing(&[1_000]);
    tie.llm_provider = "vLLM".into();
    tie.llm_kv_free_tokens = Some(130_072);
    let forecast = forecast_for(&tie, Memory::Shared).unwrap();
    assert_eq!(forecast.by_memory, Some(forecast.by_window));
    let said = lines(&tie);
    assert!(said[1].ends_with(", the smaller"), "{said:?}");
    assert!(!said[2].ends_with(", the smaller"), "{said:?}");
    // A model without KV layers: memory limits only its state, so the
    // window alone is given, and named the smaller of nothing.
    let mut flat = owner(Some(REMAINDER), &[5_000]);
    flat.model.kv_layers = Some(0);
    let forecast = forecast_for(&flat, Memory::Shared).unwrap();
    assert_eq!(
        (forecast.by_memory, forecast.tokens),
        (None, WINDOW - 5_000)
    );
    assert_eq!(
        lines(&flat)[1..],
        [
            "By window: 131.1k tokens allowed − 5.0k held = 126.1k tokens",
            "Held: 5.0k tokens in 1 request: 0 B of KV cache and ≈305.2 MiB of state",
        ]
    );
}
