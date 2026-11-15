// SPDX-License-Identifier: MIT
use super::*;
use crate::config::OmxConfig;
use crate::domain::MIB;
use crate::model_facts::{read_mlx_model, weight_shards};
use crate::test_support::{
    owner_models, reply, serve, Exchange, FakeHost, TempDir, OWNER_DRAFT, OWNER_MODEL,
};

const MODEL: &str = "Qwen3.8-27B-oQ4e-mtp";
const GLOBAL_SETTINGS: &str = include_str!("fixtures/model_facts/omlx_global_settings.json");
const MODEL_SETTINGS: &str = include_str!("fixtures/model_facts/omlx_model_settings.json");

/// `/v1/models/status` for the owner's model as oMLX 0.7.3 builds it
/// (`EnginePool.get_status` and `list_models_status`), with the sizes of
/// the recorded admin stats.
fn models_status() -> Value {
    json!({"models": [
        {"id": MODEL, "model_path": OWNER_MODEL, "loaded": true, "is_loading": false,
         "estimated_size": 17_820_265_635_u64, "actual_size": 25_744_836_152_u64,
         "external_engine": "splash", "model_type": "llm", "model_context_length": 262_144,
         "max_context_window": 131_072, "max_tokens": 32_768},
        {"id": "Qwen3.8-27B-DFlash2-incoai", "model_path": OWNER_DRAFT, "loaded": false,
         "external_engine": null, "model_context_length": 262_144, "max_context_window": null}
    ]})
}

/// The admin stats' entry for a loaded model.
fn loaded(id: &str) -> Value {
    json!({"id": id, "estimated_size": 17_820_265_635_u64, "actual_size": 25_744_836_152_u64,
           "active_requests": 0, "waiting_requests": 0})
}

fn owner_model(host: &FakeHost, dir: &str) -> MlxModel {
    read_mlx_model(host, Path::new(dir), &weight_shards(host, Path::new(dir))).unwrap()
}

#[test]
fn omlx_settings_keep_only_what_serving_needs() {
    let settings = OmlxSettings::parse(&serde_json::from_str(GLOBAL_SETTINGS).unwrap());
    assert_eq!(
        settings,
        OmlxSettings {
            base_path: Some(PathBuf::from("/omlx")),
            splash_kv_format: Some("int8".into()),
            splash_version: Some("1.5.0".into()),
        }
    );
    // The API key the response carries is not kept; text is printable.
    let settings = OmlxSettings::parse(&json!({
        "auth": {"api_key": "secret"},
        "integrations": {"splash_kv_format": "bf16\u{1b}[2J"}
    }));
    assert_eq!(settings.splash_kv_format.as_deref(), Some("bf16[2J"));
    assert!(!format!("{settings:?}").contains("secret"));
    assert_eq!(OmlxSettings::parse(&json!([])), OmlxSettings::default());
}

#[test]
fn models_status_names_each_models_files_engine_and_context() {
    let status = parse_models_status(&models_status());
    assert_eq!(
        status[0],
        ModelStatus {
            id: MODEL.into(),
            path: Some(PathBuf::from(OWNER_MODEL)),
            engine: Some("splash".into()),
            context: Some(131_072),
            trained: Some(262_144),
        }
    );
    assert_eq!((status[1].engine.clone(), status[1].context), (None, None));
    // An entry without an id is no model; only a whole path is a path.
    let odd = parse_models_status(&json!({"models": [
        {"model_path": "/x"},
        {"id": "a", "model_path": "models/a"}
    ]}));
    assert_eq!(
        odd,
        [ModelStatus {
            id: "a".into(),
            ..ModelStatus::default()
        }]
    );
    assert!(parse_models_status(&json!({})).is_empty());
}

#[test]
fn model_settings_say_how_omlx_serves_each_model() {
    let all = parse_model_settings(&serde_json::from_str(MODEL_SETTINGS).unwrap()).unwrap();
    assert_eq!(
        all[MODEL],
        ModelSettings {
            engine: Some("splash".into()),
            kv_quantized: Some("TurboQuant 8-bit".into()),
            mtp: true,
            dflash: false,
            draft: Some(PathBuf::from(OWNER_DRAFT)),
        }
    );
    assert_eq!(all["Qwen3.8-27B-DFlash2-incoai"], ModelSettings::default());
    let other = parse_model_settings(&json!({"models": {
        "a": {"turboquant_kv_enabled": true, "turboquant_kv_scheme": "affine8",
              "turboquant_kv_bits": 8, "dflash_enabled": true, "dflash_draft_model": "org/draft"},
        "b": {"turboquant_kv_enabled": true, "turboquant_kv_bits": 3.5},
        "c": {"turboquant_kv_enabled": false, "turboquant_kv_bits": 8}
    }}))
    .unwrap();
    assert_eq!(other["a"].kv_quantized.as_deref(), Some("affine 8-bit"));
    // A draft named by its repository is resolved by oMLX, not here.
    assert_eq!((other["a"].dflash, other["a"].draft.clone()), (true, None));
    assert_eq!(
        other["b"].kv_quantized.as_deref(),
        Some("TurboQuant 3.5-bit")
    );
    assert_eq!(other["c"].kv_quantized, None);
    assert_eq!(parse_model_settings(&json!({"version": 1})), None);
}

#[test]
fn the_owners_server_reads_as_measured() {
    let host = owner_models(FakeHost::default());
    let (model, draft) = (
        owner_model(&host, OWNER_MODEL),
        owner_model(&host, OWNER_DRAFT),
    );
    let status = parse_models_status(&models_status());
    let settings = OmlxSettings::parse(&serde_json::from_str(GLOBAL_SETTINGS).unwrap());
    let all = parse_model_settings(&serde_json::from_str(MODEL_SETTINGS).unwrap()).unwrap();
    let stats = loaded(MODEL);
    let facts = omlx_model_facts(&Serving {
        status: status.first(),
        settings: &settings,
        model_settings: all.get(MODEL),
        model: Some(&model),
        draft: Some(&draft),
        version: Some("0.7.3"),
        loaded: Some(&stats),
    });
    // Splash serves it, so its KV is the int8 oMLX's settings give Splash,
    // not the TurboQuant set for oMLX's own engine.
    assert_eq!(facts.bytes_per_pass, Some(15_020_479_488));
    assert_eq!(facts.kv_bytes_per_token(), Some(33_280));
    assert_eq!(facts.kv_dtype.as_deref(), Some("int8"));
    assert_eq!(facts.fixed_state_bytes.map(|bytes| bytes / MIB), Some(152));
    assert_eq!(
        (facts.draft, facts.draft_bytes),
        (Some(true), Some(3_848_808_960))
    );
    assert_eq!(
        (facts.context_configured, facts.context_trained),
        (Some(131_072), Some(262_144))
    );
    assert_eq!(
        facts.runtime_version.as_deref(),
        Some("oMLX 0.7.3 · Splash 1.5.0")
    );
    assert_eq!(
        (facts.loaded_bytes, facts.estimated_bytes),
        (Some(25_744_836_152), Some(17_820_265_635))
    );
    // A draft that does not fit the model is not the one Splash runs.
    let facts = omlx_model_facts(&Serving {
        status: status.first(),
        settings: &settings,
        model_settings: all.get(MODEL),
        model: Some(&model),
        draft: Some(&model),
        version: None,
        loaded: None,
    });
    assert_eq!((facts.draft, facts.draft_bytes), (Some(true), None));
    assert_eq!(
        facts.runtime_version.as_deref(),
        Some("oMLX · Splash 1.5.0")
    );
    assert_eq!((facts.loaded_bytes, facts.estimated_bytes), (None, None));
}

#[test]
fn omlxs_own_engine_runs_the_model_as_its_settings_say() {
    let host = owner_models(FakeHost::default());
    let model = owner_model(&host, OWNER_MODEL);
    let settings = OmlxSettings::default();
    let facts = |model_settings: Option<&ModelSettings>, model: Option<&MlxModel>| {
        omlx_model_facts(&Serving {
            status: None,
            settings: &settings,
            model_settings,
            model,
            draft: None,
            version: Some("0.7.3"),
            loaded: None,
        })
    };
    // KV in the activations' bf16; the model's MTP head drafts.
    let mtp = ModelSettings {
        mtp: true,
        ..ModelSettings::default()
    };
    let plain = facts(Some(&mtp), Some(&model));
    assert_eq!(plain.kv_bytes_per_token(), Some(65_536));
    assert_eq!(plain.fixed_state_bytes, Some(153_944_064));
    assert_eq!(
        (plain.draft, plain.draft_bytes),
        (Some(true), Some(314_297_344))
    );
    assert_eq!(plain.runtime_version.as_deref(), Some("oMLX 0.7.3"));
    // TurboQuant is named but not sized.
    let turbo = ModelSettings {
        kv_quantized: Some("TurboQuant 8-bit".into()),
        ..ModelSettings::default()
    };
    let quantized = facts(Some(&turbo), Some(&model));
    assert_eq!(quantized.kv_dtype.as_deref(), Some("TurboQuant 8-bit"));
    assert_eq!(quantized.kv_bytes_per_token(), None);
    assert_eq!(
        (quantized.draft, quantized.draft_bytes),
        (Some(false), None)
    );
    // A DFlash draft whose files are not read: drafting, size unknown.
    let dflash = ModelSettings {
        dflash: true,
        ..ModelSettings::default()
    };
    assert_eq!(facts(Some(&dflash), Some(&model)).draft, Some(true));
    // MTP without an MTP head drafts nothing; with no files, unknown.
    let headless = MlxModel {
        mtp_bytes: Some(0),
        ..model.clone()
    };
    assert_eq!(facts(Some(&mtp), Some(&headless)).draft, Some(false));
    assert_eq!(facts(Some(&mtp), None).draft, None);
    // Without the settings file neither the KV format nor the draft is known.
    let unknown = facts(None, Some(&model));
    assert_eq!((unknown.kv_dtype, unknown.draft), (None, None));
    assert_eq!(unknown.bytes_per_pass, Some(15_020_479_488));
    // Splash's KV format other than int8 or bf16 is named, unsized.
    let splash = OmlxSettings {
        splash_kv_format: Some("int4".into()),
        ..OmlxSettings::default()
    };
    let named = omlx_model_facts(&Serving {
        status: None,
        settings: &splash,
        model_settings: Some(&ModelSettings {
            engine: Some("splash".into()),
            ..ModelSettings::default()
        }),
        model: None,
        draft: None,
        version: None,
        loaded: None,
    });
    assert_eq!(named.kv_dtype.as_deref(), Some("int4"));
    assert_eq!(
        (named.kv_bytes_per_token(), named.runtime_version),
        (None, None)
    );
}

fn client(port: u16, home: &TempDir, files: &FakeHost) -> LlmTelemetryClient {
    let config = Config {
        omx: Some(OmxConfig {
            host: Some("127.0.0.1".into()),
            port: Some(port),
        }),
        ..Config::default()
    };
    let mut client = LlmTelemetryClient::from_config(&config, Some(home.0.clone()));
    client.files = Box::new(files.clone());
    client
}

#[test]
fn the_client_reads_the_served_models_facts_once_and_checks_them_each_minute() {
    let home = TempDir::new("omlx-facts");
    home.write(".config/omlx-coding/server.env", "API_KEY=facts-key\n");
    let files = owner_models(FakeHost::default()).file("/omlx/model_settings.json", MODEL_SETTINGS);
    let health = json!({"status": "healthy", "default_model": MODEL}).to_string();
    let stats = json!({
        "active_models": {"models": [loaded(MODEL)]},
        "runtime_cache": {"base_path": "/elsewhere"}
    })
    .to_string();
    let (port, server) = serve(vec![
        reply("GET /health ", 200, &health),
        Exchange {
            request: "POST /admin/api/login ",
            status: 200,
            headers: "Set-Cookie: session=abc; Path=/\r\n",
            body: "{}".into(),
        },
        reply("GET /admin/api/stats?scope=session ", 200, &stats),
        reply("GET /admin/api/device-info ", 200, "{}"),
        reply("GET /admin/api/global-settings ", 200, GLOBAL_SETTINGS),
        reply("GET /v1/models/status ", 200, &models_status().to_string()),
        reply(
            "GET /api/status ",
            200,
            r#"{"status":"ok","version":"0.7.3"}"#,
        ),
        // The next poll within the minute: what changes each second only.
        reply("GET /health ", 200, &health),
        reply("GET /admin/api/stats?scope=session ", 200, &stats),
        // The next minute: the status again, the files only checked.
        reply("GET /health ", 200, &health),
        reply("GET /admin/api/stats?scope=session ", 200, &stats),
        reply("GET /admin/api/device-info ", 200, "{}"),
        reply("GET /admin/api/global-settings ", 200, GLOBAL_SETTINGS),
        reply("GET /v1/models/status ", 200, &models_status().to_string()),
        reply("GET /api/status ", 404, "{}"),
    ]);
    let mut client = client(port, &home, &files);
    let reads = || {
        files
            .calls()
            .iter()
            .filter(|call| call.starts_with("prefix"))
            .count()
    };
    let facts = client.poll_once().expect("telemetry").model_facts;
    assert_eq!(facts.bytes_per_pass, Some(15_020_479_488));
    assert_eq!(facts.kv_bytes_per_token(), Some(33_280));
    assert_eq!(facts.fixed_state_bytes.map(|bytes| bytes / MIB), Some(152));
    assert_eq!(facts.draft_bytes, Some(3_848_808_960));
    assert_eq!(facts.context_configured, Some(131_072));
    assert_eq!(
        facts.runtime_version.as_deref(),
        Some("oMLX 0.7.3 · Splash 1.5.0")
    );
    let first = reads();
    // Model settings, the model's config and shards, the draft's.
    assert_eq!(first, 1 + 9 + 3);
    let again = client.poll_once().expect("telemetry").model_facts;
    assert_eq!(again, facts);
    client.next_metadata_poll = Instant::now();
    let checked = client.poll_once().expect("telemetry").model_facts;
    assert_eq!(checked, facts, "the version oMLX gave before is kept");
    assert_eq!(reads(), first, "nothing changed, nothing read again");
    let requests = server.join().unwrap();
    // The status endpoints take oMLX's key as a bearer token.
    assert!(requests[5].contains("Authorization: Bearer facts-key\r\n"));
    assert!(requests[6].contains("Authorization: Bearer facts-key\r\n"));
    assert!(!requests[0].contains("Authorization"));
}

#[test]
fn a_model_that_comes_in_is_looked_up_once_and_several_have_no_facts() {
    let home = TempDir::new("omlx-facts-switch");
    let files = owner_models(FakeHost::default());
    let health = json!({"status": "healthy", "default_model": "other"}).to_string();
    let other = json!({"active_models": {"models": [loaded("other")]}}).to_string();
    let both = json!({"active_models": {"models": [
        loaded(MODEL), loaded("other")
    ]}})
    .to_string();
    let busy = |id: &str| {
        let mut model = loaded(id);
        model["active_requests"] = json!(1);
        model
    };
    let two_busy = json!({"active_models": {"models": [busy(MODEL), busy("other")]}}).to_string();
    let (port, server) = serve(vec![
        // No key: the admin API refuses; status still answers on loopback.
        reply("GET /health ", 200, &health),
        reply("GET /admin/api/stats?scope=session ", 200, &other),
        reply("GET /v1/models/status ", 200, r#"{"models": []}"#),
        reply("GET /api/status ", 500, "{}"),
        // The same model, still unlisted: not asked for again.
        reply("GET /health ", 200, &health),
        reply("GET /admin/api/stats?scope=session ", 200, &other),
        // Another model takes the headline: asked for once.
        reply("GET /health ", 200, &health),
        reply("GET /admin/api/stats?scope=session ", 200, &both),
        reply("GET /v1/models/status ", 200, &models_status().to_string()),
        reply("GET /api/status ", 200, r#"{"version":"0.7.3"}"#),
        // Two models busy at once: facts belong to neither.
        reply("GET /health ", 200, &health),
        reply("GET /admin/api/stats?scope=session ", 200, &two_busy),
    ]);
    let mut client = client(port, &home, &files);
    client.session_cookie = Some("session=abc".into());
    client.next_metadata_poll = Instant::now() + Duration::from_secs(3600);
    let first = client.poll_once().expect("telemetry");
    assert_eq!(first.model.as_deref(), Some("other"));
    assert_eq!(first.model_facts.loaded_bytes, Some(25_744_836_152));
    assert_eq!(first.model_facts.bytes_per_pass, None);
    assert_eq!(client.models.version, None);
    assert_eq!(
        client.poll_once().expect("telemetry").model_facts,
        first.model_facts
    );
    let switched = client.poll_once().expect("telemetry");
    assert_eq!(switched.model.as_deref(), Some(MODEL));
    assert_eq!(switched.model_facts.bytes_per_pass, Some(15_020_479_488));
    // Without oMLX's settings the KV format Splash runs with is not known.
    assert_eq!(switched.model_facts.kv_bytes_per_token(), None);
    assert_eq!(switched.model_facts.draft, Some(true));
    let several = client.poll_once().expect("telemetry");
    assert_eq!(
        several.model.as_deref(),
        Some("2 models · Qwen3.8-27B-oQ4e-mtp")
    );
    assert_eq!(several.model_facts, ModelFacts::default());
    server.join().unwrap();
}
