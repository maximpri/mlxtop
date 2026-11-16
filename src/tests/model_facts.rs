// SPDX-License-Identifier: MIT
use super::*;
use crate::domain::{Sample, MIB};
use crate::test_support::{owner_models, FakeHost, OWNER_DRAFT, OWNER_MODEL};
use serde_json::json;
use std::time::Duration;

fn owner_model(host: &FakeHost) -> MlxModel {
    let dir = Path::new(OWNER_MODEL);
    read_mlx_model(host, dir, &weight_shards(host, dir)).expect("the owner's model reads")
}

/// A header-only safetensors file: the 8-byte length, then the header.
fn safetensors(tensors: &[(&str, &str, &[u64])]) -> Vec<u8> {
    let mut offset = 0;
    let mut header = serde_json::Map::new();
    for (name, dtype, shape) in tensors {
        let size = match *dtype {
            "U32" | "F32" => 4,
            "BF16" | "F16" => 2,
            _ => 1,
        };
        let bytes = shape.iter().product::<u64>() * size;
        header.insert(
            (*name).into(),
            json!({"dtype": dtype, "shape": shape, "data_offsets": [offset, offset + bytes]}),
        );
        offset += bytes;
    }
    let header = Value::Object(header).to_string().into_bytes();
    let mut file = (header.len() as u64).to_le_bytes().to_vec();
    file.extend(header);
    file
}

/// A one-file model in `/m` with this `config.json`.
fn model(config: Value, tensors: &[(&str, &str, &[u64])]) -> MlxModel {
    let host = FakeHost::default()
        .file("/m/config.json", &config.to_string())
        .blob("/m/model.safetensors", &safetensors(tensors))
        .dir("/m", &["/m/config.json", "/m/model.safetensors"]);
    read_mlx_model(
        &host,
        Path::new("/m"),
        &weight_shards(&host, Path::new("/m")),
    )
    .unwrap()
}

/// A small dense model: one layer, a 16-token vocabulary, 8 wide.
fn dense(tied: bool) -> MlxModel {
    let mut tensors: Vec<(&str, &str, &[u64])> = vec![
        ("model.embed_tokens.weight", "BF16", &[16, 8]),
        ("model.layers.0.self_attn.q_proj.weight", "BF16", &[8, 8]),
        ("model.norm.weight", "BF16", &[8]),
    ];
    if !tied {
        tensors.push(("lm_head.weight", "BF16", &[16, 8]));
    }
    model(
        json!({"num_hidden_layers": 1, "num_attention_heads": 2, "num_key_value_heads": 1,
               "hidden_size": 8, "torch_dtype": "bfloat16", "vocab_size": 16,
               "tie_word_embeddings": tied}),
        &tensors,
    )
}

#[test]
fn the_owners_model_reads_as_its_server_measured_it() {
    let host = owner_models(FakeHost::default());
    let model = owner_model(&host);
    // A text forward pass reads the decoder layers and lm_head: not the
    // vision tower, the MTP head or the embedding, which is only a lookup.
    let pass = model.bytes_per_pass.expect("bytes per pass");
    assert!(
        (pass as f64 / 15_020_479_488.0 - 1.0).abs() <= 0.001,
        "{pass}"
    );
    assert_eq!(pass, 14_305_317_888 + 715_161_600);
    assert_eq!(model.weight_bytes, Some(16_971_398_624));
    assert_eq!(model.mtp_bytes, Some(314_297_344));
    assert_eq!(model.parameters, Some(26_895_998_464));
    assert_eq!(model.active_parameters, Some(25_624_600_064));
    assert_eq!(model.quantization.as_deref(), Some("4/5-bit g64"));
    assert_eq!(model.context_trained, Some(262_144));
    assert_eq!(
        (model.dtype.as_deref(), model.activation_bytes),
        (Some("bf16"), Some(2))
    );
    assert_eq!((model.experts, model.experts_per_token), (None, None));
    let layout = model.layout.clone().expect("layout");
    assert_eq!(
        (layout.full_layers, layout.kv_heads, layout.head_dim),
        (16, Some(4), Some(256))
    );
    assert_eq!(
        layout.gdn,
        Some(Gdn {
            layers: 48,
            value_heads: 48,
            key_dim: 128,
            value_dim: 128,
            conv_channels: 10_240,
            conv_history: 3,
        })
    );

    // Served through Splash with int8 KV, as on the server: 32,768 int8
    // values a token and a byte of scale per 64 of them.
    let splash = mlx_facts(&model, Engine::Splash, Some(&KvFormat::SplashInt8));
    assert_eq!(splash.kv_bytes_per_token(), Some(33_280));
    assert_eq!(splash.kv_dtype.as_deref(), Some("int8"));
    // Splash's "states of 152 MiB": 48 page-aligned GDN cells (bf16
    // convolution history, fp32 recurrent state) and the draft's 5.6 MiB
    // context window, exactly what its state pool grows by per state.
    assert_eq!(splash.fixed_state_bytes, Some(154_140_672 + 5_898_240));
    assert_eq!(splash.fixed_state_bytes.unwrap() / MIB, 152);
    assert_eq!(
        splash.bits_per_weight.map(|bits| (bits * 100.0).round()),
        Some(469.0)
    );
    assert_eq!(splash.bytes_per_pass, Some(15_020_479_488));

    // oMLX's own engine keeps the same state unaligned and no draft window,
    // and bf16 KV costs twice the values.
    let mlx = mlx_facts(&model, Engine::MlxLm, Some(&KvFormat::Activations));
    assert_eq!(mlx.fixed_state_bytes, Some(153_944_064));
    assert_eq!(mlx.kv_bytes_per_token(), Some(65_536));
    assert_eq!(mlx.kv_dtype.as_deref(), Some("bf16"));
    let bf16 = mlx_facts(&model, Engine::Splash, Some(&KvFormat::Bf16));
    assert_eq!(bf16.kv_bytes_per_token(), Some(65_536));
}

#[test]
fn only_the_headers_are_read_never_the_weights() {
    let host = owner_models(FakeHost::default());
    owner_model(&host);
    let shards: Vec<String> = host
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("prefix") && call.contains(".safetensors"))
        .collect();
    assert_eq!(shards.len(), 8, "{shards:?}");
    for pair in shards.chunks(2) {
        let (first, second) = (&pair[0], &pair[1]);
        assert!(first.ends_with(" 8"), "{first}");
        let path = first.trim_start_matches("prefix ").trim_end_matches(" 8");
        let bytes = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/tests/fixtures/model_facts/Qwen3.8-27B-oQ4e-mtp")
                .join(Path::new(path).file_name().unwrap()),
        )
        .unwrap();
        // The second read stops where the header does.
        assert_eq!(*second, format!("prefix {path} {}", bytes.len()));
    }
}

#[test]
fn the_draft_fits_its_target_as_omlx_pairs_them() {
    let host = owner_models(FakeHost::default());
    let model = owner_model(&host);
    let dir = Path::new(OWNER_DRAFT);
    let draft = read_mlx_model(&host, dir, &weight_shards(&host, dir)).unwrap();
    assert_eq!(draft.weight_bytes, Some(3_848_808_960));
    assert!(draft.drafts_for(&model));
    assert!(!model.drafts_for(&model));
    let other = MlxModel {
        hidden_size: Some(4096),
        ..model.clone()
    };
    assert!(!draft.drafts_for(&other));
    // The draft has no output head of its own: no bytes per pass.
    assert_eq!(draft.bytes_per_pass, None);
    let layout = draft.layout.unwrap();
    assert_eq!(
        (layout.sliding_layers, layout.sliding_window),
        (5, Some(2048))
    );
}

#[test]
fn model_files_are_read_once_and_again_only_when_they_change() {
    let host = owner_models(FakeHost::default());
    let reads = || {
        host.calls()
            .iter()
            .filter(|call| call.starts_with("prefix"))
            .count()
    };
    let mut files = ModelFiles::default();
    let dir = Path::new(OWNER_MODEL);
    let first = files.model(&host, dir, true).expect("model");
    let once = reads();
    assert_eq!(once, 9, "config and two reads a shard");
    // Every poll uses what was read; a recheck with nothing changed too.
    for recheck in [false, false, true, false, true] {
        assert_eq!(files.model(&host, dir, recheck).as_ref(), Some(&first));
    }
    assert_eq!(reads(), once);
    // A changed file is read again, when the next recheck sees it.
    host.modified_at(
        &format!("{OWNER_MODEL}/config.json"),
        SystemTime::UNIX_EPOCH + Duration::from_secs(60),
    );
    assert_eq!(files.model(&host, dir, false).as_ref(), Some(&first));
    assert_eq!(reads(), once);
    assert_eq!(files.model(&host, dir, true).as_ref(), Some(&first));
    assert_eq!(reads(), 2 * once);
    // A few models are remembered; the oldest goes first.
    for other in ["/a", "/b", "/c", "/d"] {
        assert_eq!(files.model(&host, Path::new(other), false), None);
    }
    files.model(&host, dir, false);
    assert_eq!(reads(), 3 * once + 4);
}

#[test]
fn a_directory_that_cannot_be_read_stays_unknown_without_reading_again() {
    let host = FakeHost::default();
    let mut files = ModelFiles::default();
    let dir = Path::new("/omlx/models/gone");
    for recheck in [true, false, false, true, true, false] {
        assert_eq!(files.model(&host, dir, recheck), None);
    }
    let attempts = host
        .calls()
        .iter()
        .filter(|call| call.starts_with("prefix") || call.starts_with("read"))
        .count();
    assert_eq!(attempts, 1, "{:?}", host.calls());
    // Nothing known reads as nothing, never as zero.
    for engine in [Engine::MlxLm, Engine::Splash] {
        let facts = mlx_facts(&MlxModel::default(), engine, None);
        assert_eq!(facts, ModelFacts::default());
        assert!(lines(&facts).is_empty());
    }
}

#[test]
fn every_missing_input_stays_unknown_never_zero() {
    // The config without its shards: the layout, not the bytes.
    let host = owner_models(FakeHost::default());
    let config = owner_model(&host);
    let headless = read_mlx_model(&host, Path::new(OWNER_MODEL), &[]).unwrap();
    assert_eq!(
        (
            headless.bytes_per_pass,
            headless.parameters,
            headless.weight_bytes
        ),
        (None, None, None)
    );
    assert_eq!(headless.mtp_bytes, None);
    assert_eq!(headless.layout, config.layout);
    let facts = mlx_facts(&headless, Engine::Splash, None);
    assert_eq!(
        (
            facts.bytes_per_pass,
            facts.bits_per_weight,
            facts.parameters
        ),
        (None, None, None)
    );
    // No KV format: neither the dtype nor the bytes.
    assert_eq!(
        (facts.kv_dtype.clone(), facts.kv_bytes_per_element),
        (None, None)
    );
    assert_eq!(facts.kv_bytes_per_token(), None);
    // A format named but not sized (TurboQuant) is named, not sized.
    let turbo = mlx_facts(
        &headless,
        Engine::MlxLm,
        Some(&KvFormat::Named("TurboQuant 8-bit".into())),
    );
    assert_eq!(turbo.kv_dtype.as_deref(), Some("TurboQuant 8-bit"));
    assert_eq!(turbo.kv_bytes_per_token(), None);
    assert_eq!(turbo.fixed_state_bytes, Some(153_944_064));
    // Without the activation dtype mlx-lm's state is unknown, and Splash's
    // window without the hidden size.
    let blind = MlxModel {
        activation_bytes: None,
        hidden_size: None,
        ..headless.clone()
    };
    assert_eq!(
        mlx_facts(&blind, Engine::MlxLm, None).fixed_state_bytes,
        None
    );
    assert_eq!(
        mlx_facts(&blind, Engine::Splash, None).fixed_state_bytes,
        None
    );
    // One shard that cannot be read leaves every byte figure unknown:
    // a partial count would understate the model.
    let mut shards = weight_shards(&host, Path::new(OWNER_MODEL));
    shards.push(PathBuf::from("/omlx/models/missing.safetensors"));
    let partial = read_mlx_model(&host, Path::new(OWNER_MODEL), &shards).unwrap();
    assert_eq!((partial.bytes_per_pass, partial.weight_bytes), (None, None));
}

#[test]
fn a_tied_embedding_is_read_as_the_output_head() {
    // Tied: the embedding is the output head, read whole every pass.
    let tied = dense(true);
    assert_eq!(tied.bytes_per_pass, Some(128 + 16 + 256));
    assert_eq!(tied.parameters, Some(64 + 8 + 128));
    assert_eq!(tied.active_parameters, Some(64 + 8 + 128));
    // Untied: the embedding is only a lookup; the head is read.
    let untied = dense(false);
    assert_eq!(untied.bytes_per_pass, Some(128 + 16 + 256));
    assert_eq!(untied.parameters, Some(64 + 8 + 128 + 128));
    assert_eq!(untied.active_parameters, Some(64 + 8 + 128));
    let facts = mlx_facts(&untied, Engine::MlxLm, Some(&KvFormat::Activations));
    assert_eq!(facts.quantization.as_deref(), Some("bf16"));
    assert_eq!(facts.bits_per_weight, Some(16.0));
    // One layer, one KV head of 4: two bf16 values of 4 a token.
    assert_eq!(facts.kv_bytes_per_token(), Some(2 * 4 * 2));
    assert_eq!(facts.fixed_state_bytes, Some(0));
    // Neither head nor embedding: what a pass reads is not known.
    let headless = model(
        json!({"num_hidden_layers": 1, "num_attention_heads": 1}),
        &[("model.layers.0.mlp.up_proj.weight", "BF16", &[4, 4])],
    );
    assert_eq!(headless.bytes_per_pass, None);
    assert_eq!(headless.parameters, Some(16));
}

#[test]
fn a_moe_token_reads_only_the_experts_it_uses() {
    let tensors: [(&str, &str, &[u64]); 4] = [
        ("model.embed_tokens.weight", "BF16", &[16, 8]),
        ("model.layers.0.mlp.gate.weight", "BF16", &[8, 8]),
        (
            "model.layers.0.mlp.switch_mlp.up_proj.weight",
            "BF16",
            &[8, 4, 8],
        ),
        ("lm_head.weight", "BF16", &[16, 8]),
    ];
    let config = json!({"num_hidden_layers": 1, "num_attention_heads": 2, "hidden_size": 8,
                        "num_experts": 8, "num_experts_per_tok": 2});
    let moe = model(config.clone(), &tensors);
    assert_eq!((moe.experts, moe.experts_per_token), (Some(8), Some(2)));
    // The router, a quarter of the experts and the head.
    assert_eq!(moe.bytes_per_pass, Some(128 + 512 * 2 / 8 + 256));
    assert_eq!(moe.active_parameters, Some(64 + 256 * 2 / 8 + 128));
    assert_eq!(moe.parameters, Some(128 + 64 + 256 + 128));
    let facts = mlx_facts(&moe, Engine::MlxLm, None);
    assert_eq!(
        lines(&facts),
        ["Model: 512 B read per pass · 16.00 bits per weight · 576 parameters, 256 active"]
    );
    // Without how many experts a token uses, its bytes are unknown.
    let mut unsure = config.clone();
    unsure
        .as_object_mut()
        .unwrap()
        .remove("num_experts_per_tok");
    assert_eq!(model(unsure, &tensors).bytes_per_pass, None);
    // Experts in the config the tensors do not show: unknown, not dense.
    assert_eq!(model(config, &tensors[..1]).bytes_per_pass, None);
}

#[test]
fn packed_weights_count_the_values_they_hold() {
    let tensors: [(&str, &str, &[u64]); 5] = [
        ("model.embed_tokens.weight", "BF16", &[16, 16]),
        ("model.layers.0.mlp.up_proj.weight", "U32", &[4, 2]),
        ("model.layers.0.mlp.up_proj.scales", "BF16", &[4, 1]),
        ("model.layers.0.mlp.up_proj.biases", "BF16", &[4, 1]),
        ("lm_head.weight", "BF16", &[16, 16]),
    ];
    let config = |quantization: Value| json!({"num_hidden_layers": 1, "num_attention_heads": 1, "quantization": quantization});
    // Two words of 4-bit values a row hold 16 of them; scales are not parameters.
    let four = model(config(json!({"bits": 4, "group_size": 16})), &tensors);
    assert_eq!(four.active_parameters, Some(64 + 256));
    assert_eq!(four.quantization.as_deref(), Some("4-bit g16"));
    // A module's own bits win.
    let own = json!({"bits": 4, "group_size": 16,
                     "model.layers.0.mlp.up_proj": {"bits": 8, "group_size": 16},
                     "model.layers.0.mlp.gate": false});
    let eight = model(config(own), &tensors);
    assert_eq!(eight.active_parameters, Some(32 + 256));
    assert_eq!(eight.quantization.as_deref(), Some("4/8-bit g16"));
    // An MX format is named as such.
    let mx = model(
        config(json!({"bits": 4, "group_size": 32, "mode": "mxfp4"})),
        &tensors,
    );
    assert_eq!(mx.quantization.as_deref(), Some("mxfp4"));
    // A packed weight whose bits are not known has no parameter count,
    // though its bytes are known.
    let unknown = model(
        json!({"num_hidden_layers": 1, "num_attention_heads": 1}),
        &tensors,
    );
    assert_eq!(unknown.parameters, None);
    assert_eq!(unknown.active_parameters, None);
    assert_eq!(unknown.bytes_per_pass, Some(32 + 8 + 8 + 512));
    // The activation dtype then comes from the scales.
    assert_eq!(unknown.dtype.as_deref(), Some("bf16"));
    // A checkpoint quantized another way keeps its method's name.
    let fp8 = model(
        json!({"num_hidden_layers": 1, "quantization_config": {"quant_method": "fp8"}}),
        &tensors[..1],
    );
    assert_eq!(fp8.quantization.as_deref(), Some("fp8"));
}

#[test]
fn layer_kinds_set_what_a_sequence_keeps() {
    let layout = |text: Value| super::layout(text.as_object().unwrap());
    // Qwen3-Next: one full-attention layer in every four, from the interval.
    let next = layout(json!({
        "num_hidden_layers": 8, "full_attention_interval": 4, "num_attention_heads": 16,
        "num_key_value_heads": 2, "head_dim": 256, "linear_num_value_heads": 32,
        "linear_num_key_heads": 16, "linear_key_head_dim": 128, "linear_value_head_dim": 128,
        "linear_conv_kernel_dim": 4}))
    .unwrap();
    assert_eq!(next.full_layers, 2);
    assert_eq!(next.gdn.unwrap().layers, 6);
    // Without the linear layers' sizes the layout is not known.
    assert_eq!(
        layout(json!({"num_hidden_layers": 4, "layer_types":
            ["linear_attention", "linear_attention", "linear_attention", "full_attention"]})),
        None
    );
    // Sliding windows: only the full layers grow; the windows are state.
    let oss = layout(json!({
        "num_hidden_layers": 4, "num_attention_heads": 8, "num_key_value_heads": 2,
        "head_dim": 64, "sliding_window": 128,
        "layer_types": ["sliding_attention", "full_attention", "sliding_attention", "full_attention"]}))
    .unwrap();
    assert_eq!((oss.full_layers, oss.sliding_layers), (2, 2));
    let model = MlxModel {
        layout: Some(oss.clone()),
        activation_bytes: Some(2),
        dtype: Some("bf16".into()),
        ..MlxModel::default()
    };
    let facts = mlx_facts(&model, Engine::MlxLm, Some(&KvFormat::Activations));
    assert_eq!(facts.kv_bytes_per_token(), Some(2 * 2 * 2 * 64 * 2));
    assert_eq!(facts.fixed_state_bytes, Some(2 * 2 * 2 * 64 * 2 * 128));
    // A window of unknown size, or Splash's own window pages: unknown.
    let unbounded = MlxModel {
        layout: Some(Layout {
            sliding_window: None,
            ..oss
        }),
        ..model.clone()
    };
    let facts = mlx_facts(&unbounded, Engine::MlxLm, Some(&KvFormat::Activations));
    assert_eq!(facts.fixed_state_bytes, None);
    let facts = mlx_facts(&model, Engine::Splash, Some(&KvFormat::Bf16));
    assert_eq!(facts.fixed_state_bytes, None);
    // Latent attention (MLA) caches what the runtime chooses: unknown.
    let mla = layout(json!({"num_hidden_layers": 2, "num_attention_heads": 16,
                            "kv_lora_rank": 512, "head_dim": 192}))
    .unwrap();
    assert_eq!(
        (mla.full_layers, mla.kv_heads, mla.head_dim),
        (2, None, None)
    );
    // The head dimension follows from the hidden size where not given.
    let plain =
        layout(json!({"num_layers": 2, "num_attention_heads": 4, "hidden_size": 64})).unwrap();
    assert_eq!((plain.kv_heads, plain.head_dim), (Some(4), Some(16)));
    assert_eq!(
        layout(json!({"num_hidden_layers": 2, "num_attention_heads": 3, "hidden_size": 64}))
            .unwrap()
            .head_dim,
        None
    );
    // Kinds not understood, or as many kinds as layers, or none: unknown.
    for text in [
        json!({"num_hidden_layers": 2, "layer_types": ["mamba", "full_attention"]}),
        json!({"num_hidden_layers": 3, "layer_types": ["full_attention"]}),
        json!({"num_hidden_layers": 1, "layer_types": [1]}),
        json!({"num_hidden_layers": 1, "layer_types": "full_attention"}),
        json!({"layer_types": []}),
    ] {
        assert_eq!(layout(text.clone()), None, "{text}");
    }
}

#[test]
fn a_header_that_is_not_one_is_not_read() {
    let read = |bytes: &[u8]| {
        let host = FakeHost::default().blob("/s.safetensors", bytes);
        read_header(&host, Path::new("/s.safetensors"))
    };
    let mut good = 2_u64.to_le_bytes().to_vec();
    good.extend(b"{}");
    assert_eq!(read(&good), Some(Map::new()));
    assert_eq!(read(b"\x02\0\0"), None, "shorter than its length field");
    assert_eq!(read(&0_u64.to_le_bytes()), None, "an empty header");
    assert_eq!(
        read(&(HEADER_LIMIT + 1).to_le_bytes()),
        None,
        "too long to be one"
    );
    let mut short = 9_u64.to_le_bytes().to_vec();
    short.extend(b"{}");
    assert_eq!(read(&short), None, "cut short");
    let mut array = 2_u64.to_le_bytes().to_vec();
    array.extend(b"[]");
    assert_eq!(read(&array), None);
    let mut text = 2_u64.to_le_bytes().to_vec();
    text.extend(b"no");
    assert_eq!(read(&text), None);
    // An entry that is not a tensor spoils the count.
    for entry in [
        json!({"dtype": "BF16", "shape": [2]}),
        json!({"dtype": "BF16", "shape": [2], "data_offsets": [0]}),
        json!({"dtype": "BF16", "shape": [2], "data_offsets": [4, 0]}),
        json!({"dtype": "BF16", "shape": ["2"], "data_offsets": [0, 4]}),
        json!({"shape": [2], "data_offsets": [0, 4]}),
    ] {
        let header = json!({"__metadata__": {"format": "mlx"}, "x.weight": entry})
            .to_string()
            .into_bytes();
        let mut file = (header.len() as u64).to_le_bytes().to_vec();
        file.extend(header);
        let host = FakeHost::default().blob("/s.safetensors", &file);
        assert_eq!(
            read_tensors(&host, &[PathBuf::from("/s.safetensors")]),
            None,
            "{entry}"
        );
    }
    // No shards, or more than a model has.
    let host = FakeHost::default();
    assert_eq!(read_tensors(&host, &[]), None);
    let many = vec![PathBuf::from("/s.safetensors"); SHARD_LIMIT + 1];
    assert_eq!(read_tensors(&host, &many), None);
}

#[test]
fn the_shards_are_those_mlx_lm_loads() {
    let host = FakeHost::default()
        .dir(
            "/m",
            &[
                "/m/model-00002-of-00002.safetensors",
                "/m/consolidated.safetensors",
                "/m/model-00001-of-00002.safetensors",
                "/m/model.safetensors.index.json",
            ],
        )
        .dir("/w", &["/w/weights.safetensors", "/w/notes.txt"]);
    assert_eq!(
        weight_shards(&host, Path::new("/m")),
        [
            PathBuf::from("/m/model-00001-of-00002.safetensors"),
            PathBuf::from("/m/model-00002-of-00002.safetensors"),
        ]
    );
    assert_eq!(
        weight_shards(&host, Path::new("/w")),
        [PathBuf::from("/w/weights.safetensors")]
    );
    assert!(weight_shards(&host, Path::new("/none")).is_empty());
    // A config that is not JSON, or not an object, is no model.
    let host = FakeHost::default()
        .file("/a/config.json", "not json")
        .file("/b/config.json", "[]");
    assert_eq!(read_mlx_model(&host, Path::new("/a"), &[]), None);
    assert_eq!(read_mlx_model(&host, Path::new("/b"), &[]), None);
}

fn owner_facts() -> ModelFacts {
    let host = owner_models(FakeHost::default());
    ModelFacts {
        context_configured: Some(131_072),
        draft: Some(true),
        draft_bytes: Some(3_848_808_960),
        runtime_version: Some("oMLX 0.7.3 · Splash 1.5.0".into()),
        ..mlx_facts(
            &owner_model(&host),
            Engine::Splash,
            Some(&KvFormat::SplashInt8),
        )
    }
}

#[test]
fn diagnostics_say_each_fact_once_and_only_when_known() {
    assert_eq!(
        lines(&owner_facts()),
        [
            "Model: 14.0 GiB read per pass · 4.69 bits per weight, 4/5-bit g64 · 26.9B parameters · draft 3.6 GiB",
            "Context: KV 32.5 KiB per token, int8 · 152.6 MiB state per sequence · 131.1k tokens allowed of 262.1k trained",
            "Version: oMLX 0.7.3 · Splash 1.5.0",
        ]
    );
    assert!(lines(&ModelFacts::default()).is_empty());
    // Each part stands alone.
    let only = |facts: ModelFacts| lines(&facts);
    assert_eq!(
        only(ModelFacts {
            quantization: Some("4-bit".into()),
            draft: Some(true),
            ..ModelFacts::default()
        }),
        ["Model: 4-bit · draft"]
    );
    assert_eq!(
        only(ModelFacts {
            bits_per_weight: Some(8.5),
            parameters: Some(494_032_768),
            ..ModelFacts::default()
        }),
        ["Model: 8.50 bits per weight · 494M parameters"]
    );
    assert_eq!(
        only(ModelFacts {
            parameters: Some(512),
            draft: Some(false),
            ..ModelFacts::default()
        }),
        ["Model: 512 parameters"]
    );
    assert_eq!(
        only(ModelFacts {
            kv_dtype: Some("TurboQuant 8-bit".into()),
            fixed_state_bytes: Some(0),
            context_trained: Some(32_768),
            ..ModelFacts::default()
        }),
        ["Context: KV TurboQuant 8-bit · 32.8k tokens trained"]
    );
    assert_eq!(
        only(ModelFacts {
            kv_bytes_per_token_reported: Some(1_024),
            context_configured: Some(8_192),
            ..ModelFacts::default()
        }),
        ["Context: KV 1.0 KiB per token · 8.2k tokens allowed"]
    );
}

#[test]
fn a_runtimes_own_kv_figure_wins() {
    let facts = ModelFacts {
        kv_bytes_per_token_reported: Some(40_000),
        ..owner_facts()
    };
    assert_eq!(facts.kv_bytes_per_token(), Some(40_000));
    assert_eq!(kv_bytes_per_token(16, 4, 256, f64::NAN), None);
    assert_eq!(kv_bytes_per_token(0, 4, 256, 2.0), Some(0));
    assert_eq!(bits_per_weight(100, 0), None);
    assert_eq!(splash_draft_window_bytes(5_120), 5_898_240);
}

#[test]
fn facts_record_only_what_is_known_and_old_recordings_still_read() {
    assert_eq!(serde_json::to_string(&ModelFacts::default()).unwrap(), "{}");
    let facts = owner_facts();
    let text = serde_json::to_string(&facts).unwrap();
    assert!(!text.contains("null"), "{text}");
    assert_eq!(serde_json::from_str::<ModelFacts>(&text).unwrap(), facts);
    let sample = Sample {
        model: facts.clone(),
        ..Sample::default()
    };
    let line = serde_json::to_value(&sample).unwrap();
    assert_eq!(line["model"]["bytes_per_pass"], json!(15_020_479_488_u64));
    // A recording made before model facts reads with none.
    let old: Sample = serde_json::from_str(r#"{"updated":"12:00:00"}"#).unwrap();
    assert_eq!(old.model, ModelFacts::default());
}

#[test]
fn dtypes_and_other_embedding_tables_read_as_they_are() {
    for (dtype, named, bytes) in [
        ("float16", Some("fp16"), Some(2)),
        ("float32", Some("fp32"), Some(4)),
        ("int8", None, None),
    ] {
        let model = model(
            json!({"num_hidden_layers": 1, "num_attention_heads": 1, "torch_dtype": dtype}),
            &[("model.embed_tokens.weight", "BF16", &[4, 4])],
        );
        assert_eq!(
            (model.dtype.as_deref(), model.activation_bytes),
            (named, bytes),
            "{dtype}"
        );
    }
    // A per-layer embedding table is a lookup: parameters, not read whole.
    let layered = model(
        json!({"num_hidden_layers": 1, "num_attention_heads": 1}),
        &[
            ("model.embed_tokens.weight", "BF16", &[4, 4]),
            ("model.embed_tokens_per_layer.weight", "BF16", &[4, 8]),
            ("model.layers.0.mlp.up_proj.weight", "BF16", &[4, 4]),
        ],
    );
    assert_eq!(layered.parameters, Some(16 + 32 + 16));
    assert_eq!(layered.bytes_per_pass, Some(32 + 32));
}
