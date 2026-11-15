// SPDX-License-Identifier: MIT
//! What a model is, as far as speed and memory go: the weight bytes one
//! forward pass reads, its parameters and quantization, what a token of
//! context costs in KV cache and the state a sequence carries whatever its
//! length.
//!
//! An MLX model is read from its own files through the [`Host`] seam:
//! `config.json` and the JSON header at the start of each safetensors shard
//! (an 8-byte little-endian length, then the header), never the weights, and
//! read again only when a file changes. A figure a runtime reports itself
//! wins over one worked out here. A missing or unreadable input leaves its
//! facts unknown, never zero.
use crate::formatting;
use crate::host::Host;
use crate::logging::{diagnostics_log, log_field};
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A model's facts. Every one is optional: unknown is `None`, never zero.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct ModelFacts {
    /// Weight bytes one forward pass reads for a token: the decoder layers
    /// and the output head (the embedding table when it is the head), not a
    /// vision or audio tower, an MTP head or the embedding lookup. For MoE,
    /// the routed experts one token uses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bytes_per_pass: Option<u64>,
    /// Parameters of the language model: embedding, layers, output head.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) parameters: Option<u64>,
    /// Parameters one forward pass reads, counted as `bytes_per_pass` is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) active_parameters: Option<u64>,
    /// Bits per parameter a pass reads, scales and biases included.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bits_per_weight: Option<f64>,
    /// How the weights are stored: `4-bit`, `4/5-bit`, `mxfp4`, `bf16`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) quantization: Option<String>,
    /// MoE: routed experts per layer, and how many one token uses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) experts: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) experts_per_token: Option<u32>,
    /// Layers whose keys and values grow with the context, their KV heads
    /// and head dimension (unknown for latent attention, MLA, which a
    /// runtime may cache compressed or expanded).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) kv_layers: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) kv_heads: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) head_dim: Option<u32>,
    /// How the runtime stores the KV cache (`int8`, `bf16`), and what one
    /// value costs with its share of the scales.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) kv_dtype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) kv_bytes_per_element: Option<f64>,
    /// KV bytes per token as the runtime itself reports them: this wins
    /// over the formula, which hybrid attention can defeat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) kv_bytes_per_token_reported: Option<u64>,
    /// What a sequence holds whatever its length: linear-attention
    /// (recurrent) state, full sliding windows, and what the runtime keeps
    /// beside them. Zero for a model with none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) fixed_state_bytes: Option<u64>,
    /// Context the runtime is set to allow, and the context the model was
    /// trained for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) context_configured: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) context_trained: Option<u64>,
    /// Whether speculative decoding drafts for the model, and the draft's
    /// weight bytes as stored (a runtime may quantize it when it loads it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) draft: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) draft_bytes: Option<u64>,
    /// The runtime serving the model, with its engine: `oMLX 0.7.3 · Splash 1.5.0`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) runtime_version: Option<String>,
    /// Memory the runtime measured the loaded model to take, and what it
    /// estimated before loading it (oMLX's `actual_size`, `estimated_size`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) loaded_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) estimated_bytes: Option<u64>,
}

impl ModelFacts {
    /// KV bytes one more token of context costs: the runtime's own figure
    /// where it reports one, else worked out from the layers.
    pub(crate) fn kv_bytes_per_token(&self) -> Option<u64> {
        self.kv_bytes_per_token_reported.or_else(|| {
            kv_bytes_per_token(
                self.kv_layers?,
                self.kv_heads?,
                self.head_dim?,
                self.kv_bytes_per_element?,
            )
        })
    }
}

/// What Diagnostics says about the model: each fact once, only when known,
/// and no line without one.
pub(crate) fn lines(facts: &ModelFacts) -> Vec<String> {
    let mut model = Vec::new();
    if let Some(bytes) = facts.bytes_per_pass {
        model.push(format!("{} read per pass", formatting::bytes(bytes)));
    }
    model.extend(
        match (facts.bits_per_weight, facts.quantization.as_deref()) {
            (Some(bits), Some(label)) => Some(format!("{bits:.2} bits per weight, {label}")),
            (Some(bits), None) => Some(format!("{bits:.2} bits per weight")),
            (None, label) => label.map(str::to_owned),
        },
    );
    if let Some(parameters) = facts.parameters {
        // For MoE, the share a token uses.
        let active = facts
            .active_parameters
            .filter(|_| facts.experts.is_some())
            .map(|active| format!(", {} active", count(active)))
            .unwrap_or_default();
        model.push(format!("{} parameters{active}", count(parameters)));
    }
    if facts.draft == Some(true) {
        model.push(match facts.draft_bytes {
            Some(bytes) => format!("draft {}", formatting::bytes(bytes)),
            None => "draft".into(),
        });
    }
    let mut context = Vec::new();
    let dtype = facts
        .kv_dtype
        .as_deref()
        .map(|dtype| format!(", {dtype}"))
        .unwrap_or_default();
    match facts.kv_bytes_per_token() {
        Some(bytes) => context.push(format!("KV {} per token{dtype}", formatting::bytes(bytes))),
        None if !dtype.is_empty() => context.push(format!("KV{}", dtype.trim_start_matches(','))),
        None => {}
    }
    if let Some(state) = facts.fixed_state_bytes.filter(|state| *state > 0) {
        context.push(format!("{} state per sequence", formatting::bytes(state)));
    }
    let tokens = formatting::compact_tokens;
    context.extend(match (facts.context_configured, facts.context_trained) {
        (Some(allowed), Some(trained)) => Some(format!(
            "{} tokens allowed of {} trained",
            tokens(allowed),
            tokens(trained)
        )),
        (Some(allowed), None) => Some(format!("{} tokens allowed", tokens(allowed))),
        (None, Some(trained)) => Some(format!("{} tokens trained", tokens(trained))),
        (None, None) => None,
    });
    let version: Vec<String> = facts.runtime_version.iter().cloned().collect();
    [("Model", model), ("Context", context), ("Version", version)]
        .into_iter()
        .filter(|(_, parts)| !parts.is_empty())
        .map(|(label, parts)| format!("{label}: {}", parts.join(" · ")))
        .collect()
}

/// A parameter count: `26.9B`, `494M`.
fn count(parameters: u64) -> String {
    let parameters = parameters as f64;
    if parameters >= 1e9 {
        format!("{:.1}B", parameters / 1e9)
    } else if parameters >= 1e6 {
        format!("{:.0}M", parameters / 1e6)
    } else {
        format!("{parameters}")
    }
}

/// KV bytes per token of standard attention (multi-head, GQA or MQA):
/// keys and values for every KV head of every layer that keeps them.
pub(crate) fn kv_bytes_per_token(
    layers: u32,
    kv_heads: u32,
    head_dim: u32,
    bytes_per_element: f64,
) -> Option<u64> {
    let values = 2 * u64::from(layers) * u64::from(kv_heads) * u64::from(head_dim);
    (bytes_per_element.is_finite() && bytes_per_element > 0.0)
        .then(|| (values as f64 * bytes_per_element).round() as u64)
}

/// Bits per parameter, when `bytes` hold `parameters`.
pub(crate) fn bits_per_weight(bytes: u64, parameters: u64) -> Option<f64> {
    (parameters > 0).then(|| bytes as f64 * 8.0 / parameters as f64)
}

/// One sequence's Gated DeltaNet state: per layer, the short convolution's
/// history (`conv_bytes` a value) and the recurrent state (`state_bytes` a
/// value), each rounded up to whole `page`s where the runtime allocates in
/// pages (1 where it does not).
pub(crate) fn gdn_state_bytes(gdn: &Gdn, conv_bytes: u64, state_bytes: u64, page: u64) -> u64 {
    let page = page.max(1);
    let align = |bytes: u64| bytes.div_ceil(page) * page;
    let conv = align(u64::from(gdn.conv_history) * u64::from(gdn.conv_channels) * conv_bytes);
    let state = align(
        u64::from(gdn.value_heads)
            * u64::from(gdn.key_dim)
            * u64::from(gdn.value_dim)
            * state_bytes,
    );
    u64::from(gdn.layers) * (conv + state)
}

/// Splash's DFlash context window, kept with every sequence's state: 2,048
/// slots of the draft's context row (`width` values, the model's hidden
/// size) in 4-bit codes, with an fp16 scale and minimum per 64 values
/// (Splash 1.5 `DraftAttention.h`).
pub(crate) fn splash_draft_window_bytes(width: u32) -> u64 {
    const SLOTS: u64 = 2048;
    let width = u64::from(width);
    SLOTS * (width / 2) + SLOTS * (width / 64) * 4
}

/// Splash allocates a state cell's parts in host pages (Splash 1.5
/// `Checked.hpp`).
const SPLASH_PAGE: u64 = 16 * 1024;

/// How a runtime stores the KV cache, as far as its bytes go.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum KvFormat {
    /// The activation dtype (bf16 or fp16 for MLX models).
    Activations,
    /// bf16 whatever the activations.
    Bf16,
    /// Splash's int8 pages: a byte a value and a byte of scale per 64
    /// values (oMLX's own Splash memory plan counts them so).
    SplashInt8,
    /// A format with this name whose size is not known here (TurboQuant).
    Named(String),
}

/// The engine that runs a model, which decides how it keeps its state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Engine {
    /// mlx-lm, oMLX's own engine: a Gated DeltaNet layer keeps its
    /// convolution history in the activation dtype and its recurrent state
    /// in fp32; a sliding layer keeps its window.
    MlxLm,
    /// Splash: page-aligned state cells (convolution history in bf16,
    /// recurrent state in fp32) and the DFlash draft's context window.
    Splash,
}

/// The facts of an MLX model as `engine` runs it, with the KV cache in
/// `kv` (`None` when its format is not known).
pub(crate) fn mlx_facts(model: &MlxModel, engine: Engine, kv: Option<&KvFormat>) -> ModelFacts {
    let layout = model.layout.as_ref();
    let (kv_dtype, kv_bytes_per_element) = match kv {
        None => (None, None),
        Some(KvFormat::Activations) => (model.dtype.clone(), model.activation_bytes.map(f64::from)),
        Some(KvFormat::Bf16) => (Some("bf16".into()), Some(2.0)),
        Some(KvFormat::SplashInt8) => (Some("int8".into()), Some(1.0 + 1.0 / 64.0)),
        Some(KvFormat::Named(name)) => (Some(name.clone()), None),
    };
    let fixed_state_bytes = layout.and_then(|layout| {
        let gdn = |conv_bytes: u64, page: u64| {
            layout
                .gdn
                .map_or(0, |gdn| gdn_state_bytes(&gdn, conv_bytes, 4, page))
        };
        match engine {
            Engine::MlxLm => {
                let conv_bytes = match layout.gdn {
                    Some(_) => u64::from(model.activation_bytes?),
                    None => 0,
                };
                let window = sliding_state_bytes(layout, kv_bytes_per_element)?;
                Some(gdn(conv_bytes, 1) + window)
            }
            // Splash's sliding layers keep pages of their own, not sized here.
            Engine::Splash if layout.sliding_layers > 0 => None,
            Engine::Splash => {
                Some(gdn(2, SPLASH_PAGE) + splash_draft_window_bytes(model.hidden_size?))
            }
        }
    });
    ModelFacts {
        bytes_per_pass: model.bytes_per_pass,
        parameters: model.parameters,
        active_parameters: model.active_parameters,
        bits_per_weight: model
            .bytes_per_pass
            .zip(model.active_parameters)
            .and_then(|(bytes, parameters)| bits_per_weight(bytes, parameters)),
        quantization: model.quantization.clone(),
        experts: model.experts,
        experts_per_token: model.experts_per_token,
        kv_layers: layout.map(|layout| layout.full_layers),
        kv_heads: layout.and_then(|layout| layout.kv_heads),
        head_dim: layout.and_then(|layout| layout.head_dim),
        kv_dtype,
        kv_bytes_per_element,
        fixed_state_bytes,
        context_trained: model.context_trained,
        ..ModelFacts::default()
    }
}

/// What full sliding windows hold for one sequence: zero without sliding
/// layers, unknown when their window or the KV format is.
fn sliding_state_bytes(layout: &Layout, bytes_per_element: Option<f64>) -> Option<u64> {
    if layout.sliding_layers == 0 {
        return Some(0);
    }
    let per_token = kv_bytes_per_token(
        layout.sliding_layers,
        layout.kv_heads?,
        layout.head_dim?,
        bytes_per_element?,
    )?;
    per_token.checked_mul(layout.sliding_window?)
}

/// What an MLX model's own files say about it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct MlxModel {
    /// As in [`ModelFacts`].
    pub(crate) bytes_per_pass: Option<u64>,
    pub(crate) parameters: Option<u64>,
    pub(crate) active_parameters: Option<u64>,
    /// Every tensor in the shards: what loading the model reads.
    pub(crate) weight_bytes: Option<u64>,
    /// The multi-token-prediction head, which only MTP drafting runs.
    pub(crate) mtp_bytes: Option<u64>,
    pub(crate) quantization: Option<String>,
    pub(crate) experts: Option<u32>,
    pub(crate) experts_per_token: Option<u32>,
    pub(crate) layout: Option<Layout>,
    pub(crate) context_trained: Option<u64>,
    /// The activation dtype (`bf16`) and its bytes.
    pub(crate) dtype: Option<String>,
    pub(crate) activation_bytes: Option<u32>,
    /// What fits a draft to its target: `architectures`, the hidden and
    /// vocabulary sizes, the layers, and the target layers a draft reads.
    pub(crate) architectures: Vec<String>,
    pub(crate) hidden_size: Option<u32>,
    pub(crate) vocab_size: Option<u64>,
    pub(crate) layers: Option<u32>,
    pub(crate) target_layers: Option<u32>,
}

impl MlxModel {
    /// Whether this is a DFlash2 draft Splash pairs with `target`, as oMLX
    /// decides it: the same hidden and vocabulary sizes, and it reads as
    /// many layers as the target has.
    pub(crate) fn drafts_for(&self, target: &MlxModel) -> bool {
        self.architectures
            .iter()
            .any(|name| name == "DFlash2DraftModel")
            && self.hidden_size.is_some()
            && self.hidden_size == target.hidden_size
            && self.vocab_size.is_some()
            && self.vocab_size == target.vocab_size
            && self.target_layers.is_some()
            && self.target_layers == target.layers
    }
}

/// The layers that keep memory per sequence, from `config.json`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Layout {
    /// Layers whose keys and values grow with the context.
    pub(crate) full_layers: u32,
    /// Layers that keep only a window of keys and values, and the window.
    pub(crate) sliding_layers: u32,
    pub(crate) sliding_window: Option<u64>,
    /// KV heads and head dimension; unknown for latent attention (MLA).
    pub(crate) kv_heads: Option<u32>,
    pub(crate) head_dim: Option<u32>,
    /// Gated DeltaNet (linear-attention) layers.
    pub(crate) gdn: Option<Gdn>,
}

/// Gated DeltaNet layers: per layer a recurrent state of value heads ×
/// key dimension × value dimension, and a short convolution's history.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Gdn {
    pub(crate) layers: u32,
    pub(crate) value_heads: u32,
    pub(crate) key_dim: u32,
    pub(crate) value_dim: u32,
    /// The convolution's channels (queries, keys and values) and the
    /// tokens of history it keeps (its kernel less one).
    pub(crate) conv_channels: u32,
    pub(crate) conv_history: u32,
}

/// The largest `config.json` read, and the largest safetensors header.
const CONFIG_LIMIT: u64 = 4 * 1024 * 1024;
const HEADER_LIMIT: u64 = 16 * 1024 * 1024;
/// More shards than this and the model is not read.
const SHARD_LIMIT: usize = 512;

/// The weight shards mlx-lm loads from `dir`: `model*.safetensors`, else
/// `weight*.safetensors`; other copies (`consolidated.safetensors`) are not
/// the model it runs.
pub(crate) fn weight_shards(host: &dyn Host, dir: &Path) -> Vec<PathBuf> {
    let entries = host.read_dir(dir);
    let named = |prefix: &str| -> Vec<PathBuf> {
        let mut shards: Vec<PathBuf> = entries
            .iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".safetensors"))
            })
            .cloned()
            .collect();
        shards.sort();
        shards
    };
    let shards = named("model");
    if shards.is_empty() {
        named("weight")
    } else {
        shards
    }
}

/// The model in `dir`: what its `config.json` says, and what the headers of
/// its weight shards add. `None` when the config cannot be read.
pub(crate) fn read_mlx_model(host: &dyn Host, dir: &Path, shards: &[PathBuf]) -> Option<MlxModel> {
    let bytes = host.read_prefix(&dir.join("config.json"), CONFIG_LIMIT)?;
    let config: Value = serde_json::from_slice(&bytes).ok()?;
    let config = config.as_object()?;
    let mut model = parse_config(config);
    if let Some(tensors) = read_tensors(host, shards) {
        add_tensors(&mut model, config, &tensors);
    }
    Some(model)
}

/// The text model's settings: `text_config` (or `language_config`) in a
/// multimodal checkpoint, the top level otherwise.
fn text_config(config: &Map<String, Value>) -> &Map<String, Value> {
    ["text_config", "language_config"]
        .iter()
        .find_map(|key| config.get(*key).and_then(Value::as_object))
        .unwrap_or(config)
}

/// The first of `keys` that holds a whole number, in `text` and then at the
/// top level.
fn number(config: &Map<String, Value>, keys: &[&str]) -> Option<u64> {
    [text_config(config), config].into_iter().find_map(|map| {
        keys.iter()
            .find_map(|key| map.get(*key).and_then(Value::as_u64))
    })
}

fn small(config: &Map<String, Value>, keys: &[&str]) -> Option<u32> {
    number(config, keys).and_then(|value| u32::try_from(value).ok())
}

/// A dtype's short name and bytes.
fn dtype(name: &str) -> Option<(&'static str, u32)> {
    match name.to_ascii_lowercase().as_str() {
        "bfloat16" | "bf16" => Some(("bf16", 2)),
        "float16" | "fp16" | "f16" | "half" => Some(("fp16", 2)),
        "float32" | "fp32" | "f32" | "float" => Some(("fp32", 4)),
        _ => None,
    }
}

fn parse_config(config: &Map<String, Value>) -> MlxModel {
    let text = text_config(config);
    let activation = ["dtype", "torch_dtype"]
        .iter()
        .find_map(|key| {
            [text, config]
                .into_iter()
                .find_map(|map| map.get(*key).and_then(Value::as_str))
        })
        .and_then(dtype);
    let quantization = Quantization::parse(config);
    MlxModel {
        quantization: match &quantization {
            Some(quantization) => quantization.label(),
            None => activation.map(|(name, _)| name.to_owned()),
        },
        experts: small(
            config,
            &[
                "num_experts",
                "num_local_experts",
                "n_routed_experts",
                "moe_num_experts",
            ],
        )
        .filter(|experts| *experts > 0),
        experts_per_token: small(
            config,
            &["num_experts_per_tok", "num_experts_per_token", "moe_topk"],
        )
        .filter(|used| *used > 0),
        layout: layout(text),
        context_trained: number(
            config,
            &[
                "max_position_embeddings",
                "max_sequence_length",
                "max_seq_len",
                "seq_length",
                "n_positions",
            ],
        )
        .filter(|tokens| *tokens > 0),
        dtype: activation.map(|(name, _)| name.to_owned()),
        activation_bytes: activation.map(|(_, bytes)| bytes),
        architectures: config
            .get("architectures")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        hidden_size: small(config, &["hidden_size"]),
        vocab_size: number(config, &["vocab_size"]),
        layers: small(config, &["num_hidden_layers"]),
        target_layers: small(config, &["num_target_layers"]),
        ..MlxModel::default()
    }
}

/// Which layers keep what. `None` when a layer's kind is not one this
/// understands: a guess would put a wrong figure where none belongs.
fn layout(text: &Map<String, Value>) -> Option<Layout> {
    let get = |key: &str| {
        text.get(key)
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
    };
    let layers = get("num_hidden_layers").or_else(|| get("num_layers"))?;
    let kinds: Vec<&str> = match text.get("layer_types") {
        Some(kinds) => kinds
            .as_array()?
            .iter()
            .map(Value::as_str)
            .collect::<Option<_>>()?,
        // Qwen3-Next: one full-attention layer in every interval.
        None => match get("full_attention_interval").filter(|every| *every > 0) {
            Some(every) if text.contains_key("linear_num_value_heads") => (1..=layers)
                .map(|layer| {
                    if layer % every == 0 {
                        "full_attention"
                    } else {
                        "linear_attention"
                    }
                })
                .collect(),
            _ => vec!["full_attention"; layers as usize],
        },
    };
    if kinds.len() != layers as usize {
        return None;
    }
    let count = |kind: &str| kinds.iter().filter(|each| **each == kind).count() as u32;
    let (full, sliding, linear) = (
        count("full_attention"),
        count("sliding_attention"),
        count("linear_attention"),
    );
    if full + sliding + linear != layers {
        return None;
    }
    // Latent attention caches what the runtime chooses; not sized here.
    let latent = text.get("kv_lora_rank").is_some_and(|rank| !rank.is_null());
    let heads = get("num_attention_heads");
    let kv_heads = get("num_key_value_heads").or(heads).filter(|_| !latent);
    let head_dim = get("head_dim")
        .filter(|dim| *dim > 0)
        .or_else(|| {
            let (hidden, heads) = (get("hidden_size")?, heads?);
            (heads > 0 && hidden % heads == 0).then(|| hidden / heads)
        })
        .filter(|_| !latent);
    let gdn = if linear == 0 {
        None
    } else {
        let (key_heads, key_dim) = (get("linear_num_key_heads")?, get("linear_key_head_dim")?);
        let (value_heads, value_dim) = (
            get("linear_num_value_heads")?,
            get("linear_value_head_dim")?,
        );
        Some(Gdn {
            layers: linear,
            value_heads,
            key_dim,
            value_dim,
            conv_channels: 2 * key_heads * key_dim + value_heads * value_dim,
            conv_history: get("linear_conv_kernel_dim")?.checked_sub(1)?,
        })
    };
    Some(Layout {
        full_layers: full,
        sliding_layers: sliding,
        sliding_window: text.get("sliding_window").and_then(Value::as_u64),
        kv_heads,
        head_dim,
        gdn,
    })
}

/// How the weights are quantized, from `quantization` (mlx-lm) or
/// `quantization_config`: the default bits and mode, and a module's own.
#[derive(Clone, Debug, Default, PartialEq)]
struct Quantization {
    bits: Option<u64>,
    /// Weights that share a scale and bias.
    group_size: Option<u64>,
    mode: Option<String>,
    method: Option<String>,
    /// A module's own bits; `None` for a module left unquantized.
    modules: HashMap<String, Option<u64>>,
}

impl Quantization {
    fn parse(config: &Map<String, Value>) -> Option<Self> {
        let block = ["quantization", "quantization_config"]
            .iter()
            .find_map(|key| config.get(*key).and_then(Value::as_object))?;
        let modules = block
            .iter()
            .filter_map(|(module, value)| match value {
                Value::Object(own) => Some((module.clone(), own.get("bits")?.as_u64())),
                Value::Bool(false) => Some((module.clone(), None)),
                _ => None,
            })
            .collect();
        Some(Self {
            bits: block.get("bits").and_then(Value::as_u64),
            group_size: block.get("group_size").and_then(Value::as_u64),
            mode: block.get("mode").and_then(Value::as_str).map(str::to_owned),
            method: block
                .get("quant_method")
                .and_then(Value::as_str)
                .map(str::to_owned),
            modules,
        })
    }

    /// The bits a module's packed weights use.
    fn bits(&self, module: &str) -> Option<u64> {
        match self.modules.get(module) {
            Some(own) => *own,
            None => self.bits,
        }
    }

    /// `4-bit g64` (4-bit in groups of 64), `4/5-bit g64` where modules
    /// differ, or the format's own name (`mxfp4`, whose group is fixed).
    fn label(&self) -> Option<String> {
        if let Some(mode) = self.mode.as_deref().filter(|mode| *mode != "affine") {
            return Some(mode.to_owned());
        }
        let widths: BTreeSet<u64> = self
            .bits
            .into_iter()
            .chain(self.modules.values().flatten().copied())
            .collect();
        if widths.is_empty() {
            return self.method.clone();
        }
        let widths: Vec<String> = widths.iter().map(u64::to_string).collect();
        let group = self
            .group_size
            .map(|size| format!(" g{size}"))
            .unwrap_or_default();
        Some(format!("{}-bit{group}", widths.join("/")))
    }
}

/// One tensor in a safetensors header.
#[derive(Clone, Debug, PartialEq)]
struct Tensor {
    dtype: String,
    shape: Vec<u64>,
    bytes: u64,
}

/// The JSON header of one shard: its first 8 bytes give the header's
/// length, and nothing past the header is read.
fn read_header(host: &dyn Host, path: &Path) -> Option<Map<String, Value>> {
    let prefix = host.read_prefix(path, 8)?;
    let length = u64::from_le_bytes(prefix.get(..8)?.try_into().ok()?);
    if length == 0 || length > HEADER_LIMIT {
        return None;
    }
    let bytes = host.read_prefix(path, 8 + length)?;
    let header = bytes
        .get(8..)
        .filter(|header| header.len() as u64 == length)?;
    match serde_json::from_slice(header).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

/// Every tensor of every shard; `None` when a shard or one of its entries
/// cannot be read, since a partial count would understate the model.
fn read_tensors(host: &dyn Host, shards: &[PathBuf]) -> Option<HashMap<String, Tensor>> {
    if shards.is_empty() || shards.len() > SHARD_LIMIT {
        return None;
    }
    let mut tensors = HashMap::new();
    for shard in shards {
        for (name, entry) in read_header(host, shard)? {
            if name == "__metadata__" {
                continue;
            }
            let offsets = entry.get("data_offsets")?.as_array()?;
            let [start, end] = offsets.as_slice() else {
                return None;
            };
            let tensor = Tensor {
                dtype: entry.get("dtype")?.as_str()?.to_owned(),
                shape: entry
                    .get("shape")?
                    .as_array()?
                    .iter()
                    .map(Value::as_u64)
                    .collect::<Option<_>>()?,
                bytes: end.as_u64()?.checked_sub(start.as_u64()?)?,
            };
            tensors.insert(name, tensor);
        }
    }
    Some(tensors)
}

/// What a tensor belongs to, for what a forward pass reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Part {
    /// A vision or audio tower: text tokens never touch it.
    Media,
    /// A multi-token-prediction head: run only to draft.
    Mtp,
    /// The token embedding: a lookup, unless it is also the output head.
    Embedding,
    /// Another embedding table (per-layer embeddings): a lookup.
    Lookup,
    OutputHead,
    /// MoE experts, of which a token uses a few.
    Experts,
    /// Everything else a pass reads: layers and norms.
    Layers,
}

fn part(name: &str) -> Part {
    const MEDIA: [&str; 9] = [
        "vision_tower",
        "vision_model",
        "visual",
        "multi_modal_projector",
        "mm_projector",
        "audio_tower",
        "audio_model",
        "embed_vision",
        "embed_audio",
    ];
    let segments: Vec<&str> = name.split('.').collect();
    let has = |wanted: &[&str]| segments.iter().any(|segment| wanted.contains(segment));
    if has(&MEDIA) {
        Part::Media
    } else if has(&["mtp"]) {
        Part::Mtp
    } else if has(&["embed_tokens", "wte", "tok_embeddings", "word_embeddings"]) {
        Part::Embedding
    } else if segments.iter().any(|segment| segment.starts_with("embed")) {
        Part::Lookup
    } else if has(&["lm_head"]) {
        Part::OutputHead
    } else if has(&["switch_mlp", "experts"]) {
        Part::Experts
    } else {
        Part::Layers
    }
}

/// A tensor's parameters: its elements, or for a packed quantized weight
/// the values packed into them. A quantized module's scales and biases are
/// not parameters. `None` when a packed weight's bits are not known.
fn parameters(
    name: &str,
    tensor: &Tensor,
    tensors: &HashMap<String, Tensor>,
    quantization: Option<&Quantization>,
) -> Option<u64> {
    let (module, leaf) = name.rsplit_once('.').unwrap_or(("", name));
    let packed = tensors
        .get(&format!("{module}.weight"))
        .is_some_and(|weight| weight.dtype == "U32")
        && tensors.contains_key(&format!("{module}.scales"));
    if packed && matches!(leaf, "scales" | "biases") {
        return Some(0);
    }
    let elements = tensor
        .shape
        .iter()
        .try_fold(1_u64, |count, dimension| count.checked_mul(*dimension))?;
    if packed && leaf == "weight" {
        let bits = quantization?.bits(module).filter(|bits| *bits > 0)?;
        let values = elements.checked_mul(32)?;
        return (values % bits == 0).then(|| values / bits);
    }
    Some(elements)
}

/// Bytes and parameters by part.
#[derive(Clone, Copy, Debug, Default)]
struct Sum {
    tensors: u64,
    bytes: u64,
    parameters: Option<u64>,
}

fn add_tensors(
    model: &mut MlxModel,
    config: &Map<String, Value>,
    tensors: &HashMap<String, Tensor>,
) {
    let quantization = Quantization::parse(config);
    let mut sums: HashMap<Part, Sum> = HashMap::new();
    let mut all = Sum {
        parameters: Some(0),
        ..Sum::default()
    };
    for (name, tensor) in tensors {
        let count = parameters(name, tensor, tensors, quantization.as_ref());
        let sum = sums.entry(part(name)).or_insert(Sum {
            parameters: Some(0),
            ..Sum::default()
        });
        for sum in [sum, &mut all] {
            sum.tensors += 1;
            sum.bytes = sum.bytes.saturating_add(tensor.bytes);
            sum.parameters = sum
                .parameters
                .zip(count)
                .map(|(sum, count)| sum.saturating_add(count));
        }
    }
    let get = |part: Part| {
        sums.get(&part).copied().unwrap_or(Sum {
            parameters: Some(0),
            ..Sum::default()
        })
    };
    let (layers, experts, head) = (get(Part::Layers), get(Part::Experts), {
        let head = get(Part::OutputHead);
        // A model without its own output head reads the embedding as one.
        if head.tensors > 0 {
            head
        } else {
            get(Part::Embedding)
        }
    });
    model.weight_bytes = Some(all.bytes);
    model.mtp_bytes = Some(get(Part::Mtp).bytes);
    // A token reads `used` of every layer's `routed` experts. Experts the
    // config names but the tensors do not show leave the share unknown.
    let (routed, used) = (model.experts, model.experts_per_token);
    let share = |total: u64| -> Option<u64> {
        if experts.tensors == 0 {
            return routed.is_none().then_some(0);
        }
        let (routed, used) = (u128::from(routed?), u128::from(used?));
        u64::try_from(u128::from(total) * used / routed).ok()
    };
    if head.tensors > 0 {
        model.bytes_per_pass = share(experts.bytes).map(|routed| {
            layers
                .bytes
                .saturating_add(routed)
                .saturating_add(head.bytes)
        });
        model.active_parameters = experts
            .parameters
            .and_then(share)
            .zip(layers.parameters.zip(head.parameters))
            .map(|(routed, (layers, head))| layers.saturating_add(routed).saturating_add(head));
    }
    model.parameters = [
        Part::Layers,
        Part::Experts,
        Part::Embedding,
        Part::Lookup,
        Part::OutputHead,
    ]
    .into_iter()
    .try_fold(0_u64, |total, part| {
        Some(total.saturating_add(get(part).parameters?))
    });
    // A quantized MLX model computes in its scales' dtype.
    let scales = tensors
        .iter()
        .find(|(name, _)| name.ends_with(".scales"))
        .and_then(|(_, scales)| dtype(&scales.dtype));
    if let (None, Some((name, bytes))) = (&model.dtype, scales) {
        model.dtype = Some(name.to_owned());
        model.activation_bytes = Some(bytes);
    }
}

/// When each of a model's files last changed: what says it must be read
/// again.
type Stamp = Vec<(PathBuf, Option<SystemTime>)>;

fn stamp(host: &dyn Host, dir: &Path, shards: &[PathBuf]) -> Stamp {
    std::iter::once(dir.join("config.json"))
        .chain(shards.iter().cloned())
        .map(|path| {
            let at = host.modified(&path);
            (path, at)
        })
        .collect()
}

/// Models read from their files, each read again only when one of its
/// files changed: a monitor polls every second, and a model's files
/// almost never change.
#[derive(Debug, Default)]
pub(crate) struct ModelFiles {
    read: Vec<(PathBuf, Stamp, Option<MlxModel>)>,
}

/// Models remembered at once: the one serving, its draft, and a switch.
const REMEMBERED: usize = 4;

impl ModelFiles {
    /// The model in `dir`. Its files are read the first time it is asked
    /// for, and again only when `recheck` finds one of them changed; a
    /// directory that cannot be read stays unknown until then.
    pub(crate) fn model(&mut self, host: &dyn Host, dir: &Path, recheck: bool) -> Option<MlxModel> {
        let known = self.read.iter().position(|(read, _, _)| read == dir);
        let shards = match known {
            Some(at) if !recheck => return self.read[at].2.clone(),
            _ => weight_shards(host, dir),
        };
        let stamp = stamp(host, dir, &shards);
        if let Some(at) = known {
            if self.read[at].1 == stamp {
                return self.read[at].2.clone();
            }
            self.read.remove(at);
        }
        let model = read_mlx_model(host, dir, &shards);
        diagnostics_log(
            if model.is_some() { "INFO" } else { "WARN" },
            "model_files",
            format!(
                "dir={} read={} shards={} bytes_per_pass={}",
                log_field(&dir.display().to_string()),
                model.is_some(),
                shards.len(),
                model
                    .as_ref()
                    .and_then(|model| model.bytes_per_pass)
                    .map_or_else(|| "-".to_owned(), |bytes| bytes.to_string())
            ),
        );
        if self.read.len() >= REMEMBERED {
            self.read.remove(0);
        }
        self.read.push((dir.to_owned(), stamp, model.clone()));
        model
    }
}

#[cfg(test)]
#[path = "tests/model_facts.rs"]
mod tests;
