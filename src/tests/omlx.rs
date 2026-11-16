// SPDX-License-Identifier: MIT
use super::*;
use crate::config::OmxConfig;
use crate::domain::MIB;
use crate::model_facts::{read_mlx_model, weight_shards};
use crate::test_support::{
    on_this_host, owner_models, reply, serve, Exchange, FakeHost, TempDir, OWNER_DRAFT, OWNER_MODEL,
};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

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
            max_concurrent_requests: Some(1),
        }
    );
    // A limit of none is no limit read.
    let unset = OmlxSettings::parse(&json!({"scheduler": {"max_concurrent_requests": 0}}));
    assert_eq!(unset.max_concurrent_requests, None);
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
    let all = parse_model_settings(&serde_json::from_str(&on_this_host(MODEL_SETTINGS)).unwrap())
        .unwrap();
    assert_eq!(
        all[MODEL],
        ModelSettings {
            engine: Some("splash".into()),
            kv_quantized: Some("TurboQuant 8-bit".into()),
            mtp: true,
            dflash: false,
            draft: Some(PathBuf::from(OWNER_DRAFT)),
            vlm_mtp: false,
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
    let files = owner_models(FakeHost::default())
        .file("/omlx/model_settings.json", &on_this_host(MODEL_SETTINGS));
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
    assert_eq!(client.models.server.version, None);
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

/// oMLX 0.7.3's `/api/status` and `/v1/models/status` on the owner's server, redacted (model
/// paths under `/omlx`).
const API_STATUS: &str = include_str!("fixtures/omlx_api_status.json");
const OWNER_MODELS_STATUS: &str = include_str!("fixtures/omlx_models_status.json");

#[test]
fn the_owners_status_endpoints_read_as_served() {
    let server = ServerStatus::parse(&serde_json::from_str(API_STATUS).unwrap());
    assert_eq!(
        server,
        ServerStatus {
            version: Some("0.7.3".into()),
            ..ServerStatus::default()
        }
    );
    assert_eq!(server.slowing(MODEL, true), None);
    let status =
        parse_models_status(&serde_json::from_str(&on_this_host(OWNER_MODELS_STATUS)).unwrap());
    assert_eq!(status.len(), 4);
    let served = status.iter().find(|status| status.id == MODEL).unwrap();
    assert_eq!(
        served,
        &ModelStatus {
            id: MODEL.into(),
            path: Some(PathBuf::from(on_this_host(
                "/omlx/models/Jundot/Qwen3.8-27B-oQ4e-mtp"
            ))),
            engine: Some("splash".into()),
            context: Some(131_072),
            trained: Some(262_144),
        }
    );
}

#[test]
fn api_status_names_missing_kernels_and_shed_prefill_never_their_errors() {
    let mut status: Value = serde_json::from_str(API_STATUS).unwrap();
    status["custom_kernels"]["attention"] =
        json!({"available": false, "import_error": "dlopen /Users/someone/_ext.so failed"});
    status["custom_kernels"]["lowbit"]["available"] = json!(false);
    status["ane_prefill"]["models"] = json!([
        {"model_id": MODEL, "attempted": true, "configured": true, "shed": true},
        {"model_id": "other", "attempted": true, "configured": true, "shed": false}
    ]);
    let server = ServerStatus::parse(&status);
    assert_eq!(server.missing_kernels, ["attention", "lowbit"]);
    assert_eq!(server.shed_prefill, [MODEL]);
    assert!(!format!("{server:?}").contains("dlopen"));
    assert_eq!(
        server.slowing(MODEL, false).as_deref(),
        Some("custom kernels not loaded: attention, lowbit · Neural Engine prefill shed: prompts read on the GPU alone")
    );
    // Splash runs on kernels of its own; another model's prefill is not this one's.
    assert_eq!(server.slowing("other", true), None);
}

#[test]
fn the_client_says_what_slows_omlxs_own_engine() {
    let home = TempDir::new("omlx-slowing");
    home.write(".config/omlx-coding/server.env", "API_KEY=slow-key\n");
    let health = json!({"status": "healthy", "default_model": MODEL}).to_string();
    let stats = json!({"active_models": {"models": [loaded(MODEL)]}}).to_string();
    let mut own = models_status();
    own["models"][0]["external_engine"] = Value::Null;
    let mut api = serde_json::from_str::<Value>(API_STATUS).unwrap();
    api["custom_kernels"]["attention"]["available"] = json!(false);
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
        reply("GET /admin/api/global-settings ", 200, "{}"),
        reply("GET /v1/models/status ", 200, &own.to_string()),
        reply("GET /api/status ", 200, &api.to_string()),
        // Served through Splash, its own kernels run it.
        reply("GET /health ", 200, &health),
        reply("GET /admin/api/stats?scope=session ", 200, &stats),
        reply("GET /admin/api/device-info ", 200, "{}"),
        reply("GET /admin/api/global-settings ", 200, "{}"),
        reply("GET /v1/models/status ", 200, &models_status().to_string()),
        reply("GET /api/status ", 200, r#"{"status": "ok"}"#),
    ]);
    let mut client = client(port, &home, &FakeHost::default());
    let first = client.poll_once().expect("telemetry");
    assert_eq!(
        first.details.as_deref(),
        Some("custom kernels not loaded: attention")
    );
    assert_eq!(client.models.server.version.as_deref(), Some("0.7.3"));
    client.next_metadata_poll = Instant::now();
    let splash = client.poll_once().expect("telemetry");
    assert_eq!(splash.details, None);
    assert_eq!(
        client.models.server.version.as_deref(),
        Some("0.7.3"),
        "a status without a version keeps the one given before"
    );
    let requests = server.join().unwrap();
    // The API key alone, as a bearer token: no admin session.
    assert!(requests[6].contains("Authorization: Bearer slow-key\r\n"));
    assert!(!requests[6].contains("Cookie"));
}

#[test]
fn diagnostics_say_why_tokens_per_pass_are_missing_under_omlx() {
    let mut hosted = Sample {
        llm_provider: "oMLX".into(),
        llm_engine: Some("Splash".into()),
        ..Sample::default()
    };
    assert_eq!(
        draft_lines(&hosted),
        ["Draft: tokens per verify pass unknown · oMLX keeps Splash's status private"]
    );
    // Known, no draft, or Splash on its own: nothing to explain.
    hosted.llm_tokens_per_pass = Some(7.8);
    assert!(draft_lines(&hosted).is_empty());
    hosted.llm_tokens_per_pass = None;
    hosted.model.draft = Some(false);
    assert!(draft_lines(&hosted).is_empty());
    let alone = Sample {
        llm_provider: "Splash".into(),
        ..Sample::default()
    };
    assert!(draft_lines(&alone).is_empty());
}

#[test]
fn the_request_shown_gets_tokens_per_pass_only_while_it_writes_alone() {
    let writing = RequestUsage {
        provider: "Splash".into(),
        id: "1".into(),
        prompt: 900,
        output: Some(120),
        ..RequestUsage::default()
    };
    let mut sample = Sample {
        llm_source: TelemetrySource::Live,
        llm_status: "generating".into(),
        llm_requests: vec![writing.clone()],
        llm_tokens_per_pass: Some(7.84),
        ..Sample::default()
    };
    assert_eq!(draft_note(&sample, &writing, true), "7.8 tok/pass");
    // A request no longer live, or the counters stale: not this request's now.
    assert_eq!(draft_note(&sample, &writing, false), "");
    sample.llm_status = "stale".into();
    assert_eq!(draft_note(&sample, &writing, true), "");
    sample.llm_status = "generating".into();
    // Prefilling or queued, it writes nothing; beside another writer, the counters are shared.
    let prefilling = RequestUsage {
        prefilled: Some(400),
        ..writing.clone()
    };
    assert_eq!(draft_note(&sample, &prefilling, true), "");
    let queued = RequestUsage {
        queued: true,
        ..writing.clone()
    };
    assert_eq!(draft_note(&sample, &queued, true), "");
    sample.llm_requests.push(RequestUsage {
        id: "2".into(),
        ..writing.clone()
    });
    assert_eq!(draft_note(&sample, &writing, true), "");
    // Unknown is not shown.
    sample.llm_requests.pop();
    sample.llm_tokens_per_pass = None;
    assert_eq!(draft_note(&sample, &writing, true), "");
}

#[test]
fn without_a_settings_file_omlx_runs_each_model_on_its_defaults() {
    let path = Path::new("/omlx/model_settings.json");
    // oMLX's base directory, which holds its settings, and no file of model settings: every
    // model on oMLX's own engine with no draft.
    let base = FakeHost::default().dir("/omlx", &["/omlx/settings.json", "/omlx/models"]);
    let mut models = OmlxModels::default();
    assert_eq!(
        models.model_settings(&base, path, true),
        Some(&HashMap::new())
    );
    let facts = omlx_model_facts(&Serving {
        status: None,
        settings: &OmlxSettings::default(),
        model_settings: Some(&ModelSettings::default()),
        model: None,
        draft: None,
        version: None,
        loaded: None,
    });
    assert_eq!(facts.draft, Some(false));
    // A directory that cannot be read says nothing.
    let mut unread = OmlxModels::default();
    assert_eq!(
        unread.model_settings(&FakeHost::default(), path, true),
        None
    );
    // A vision model's assistant drafts; its files are not read.
    let all = parse_model_settings(&json!({"models": {"v": {"vlm_mtp_enabled": true}}})).unwrap();
    assert!(all["v"].vlm_mtp);
    let facts = omlx_model_facts(&Serving {
        status: None,
        settings: &OmlxSettings::default(),
        model_settings: all.get("v"),
        model: None,
        draft: None,
        version: None,
        loaded: None,
    });
    assert_eq!((facts.draft, facts.draft_bytes), (Some(true), None));
}

#[test]
fn the_client_says_a_model_on_omlxs_own_engine_without_settings_has_no_draft() {
    let home = TempDir::new("omlx-defaults");
    home.write(".config/omlx-coding/server.env", "API_KEY=defaults-key\n");
    let files = owner_models(FakeHost::default()).dir("/omlx", &["/omlx/settings.json"]);
    let health = json!({"status": "healthy", "default_model": MODEL}).to_string();
    let stats = json!({"active_models": {"models": [loaded(MODEL)]}}).to_string();
    let mut own = models_status();
    own["models"][0]["external_engine"] = Value::Null;
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
        reply("GET /v1/models/status ", 200, &own.to_string()),
        reply("GET /api/status ", 200, API_STATUS),
    ]);
    let mut client = client(port, &home, &files);
    let facts = client.poll_once().expect("telemetry").model_facts;
    assert_eq!((facts.draft, facts.draft_bytes), (Some(false), None));
    // Its own engine keeps KV in the activations' bf16.
    assert_eq!(facts.kv_bytes_per_token(), Some(65_536));
    server.join().unwrap();
}

/// A loopback server that answers each request by its path until `stop`,
/// and returns the request lines it was sent.
fn listening(answer: fn(&str) -> (u16, &'static str)) -> (u16, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let mut lines = Vec::new();
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut head = Vec::new();
            let mut byte = [0; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            let head = String::from_utf8_lossy(&head);
            let line = head.lines().next().unwrap_or_default().to_owned();
            if line.starts_with("STOP ") {
                break;
            }
            let (status, body) = answer(line.split_whitespace().nth(1).unwrap_or_default());
            let _ = write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            lines.push(line);
        }
        lines
    });
    (port, server)
}

fn stop(port: u16, server: thread::JoinHandle<Vec<String>>) -> Vec<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(b"STOP / HTTP/1.1\r\n\r\n").unwrap();
    server.join().unwrap()
}

/// A client that found its endpoint the way a stock install has it, in a
/// file, with no `omx` host or port set: nothing chose oMLX.
fn discovering(port: u16, home: &TempDir, key: &str) -> LlmTelemetryClient {
    home.write(
        ".config/omlx-coding/server.env",
        &format!("HOST=127.0.0.1\nPORT={port}\n{key}"),
    );
    let client = LlmTelemetryClient::from_config(&Config::default(), Some(home.0.clone()));
    assert_eq!((client.host.as_str(), client.port), ("127.0.0.1", port));
    client
}

#[test]
fn a_server_no_omlx_process_was_found_for_is_asked_nothing() {
    // SGLang on oMLX's port, its process out of sight in a container: its
    // `/health` generates a token unless told otherwise.
    let (port, server) = listening(|path| match path {
        "/health" | "/health_generate" => (200, ""),
        "/v1/models" => (
            200,
            r#"{"object":"list","data":[{"id":"Qwen/Qwen3-8B","object":"model","owned_by":"sglang","root":"Qwen/Qwen3-8B","max_model_len":40960}]}"#,
        ),
        "/metrics" => (200, "sglang:num_running_reqs 0\n"),
        _ => (404, r#"{"detail":"Not Found"}"#),
    });
    let home = TempDir::new("omlx-not-found");
    let mut client = discovering(port, &home, "API_KEY=never-sent\n");
    for _ in 0..5 {
        client.next_poll = Instant::now();
        assert!(client.poll(None).is_none());
    }
    let requests = stop(port, server);
    let health = requests.iter().filter(|line| line.contains(" /health"));
    assert_eq!(health.count(), 0, "{requests:?}");
    assert!(requests.is_empty(), "{requests:?}");
    // Diagnostics say no runtime was found, and ask for nothing.
    let report = client.report();
    assert_eq!(report.provider, None);
    assert_eq!(report.status(), "No runtime detected");
    assert_eq!(report.selection, "process detection");
    assert!(report.endpoint.is_empty() && report.probes.is_empty());
    assert!(report.credentials_present);
    let lines = report.lines(&Sample::default());
    assert_eq!(
        lines[..3],
        [
            "Runtime: none · No runtime detected",
            "Selection: process detection",
            "Endpoint: —"
        ]
    );
    assert!(!lines.iter().any(|line| line.contains("/health")));
}

#[test]
fn an_omlx_found_after_mlxtop_started_is_asked_at_once() {
    let (port, server) = listening(|path| match path {
        "/health" => (
            200,
            r#"{"status":"healthy","default_model":"qwen","engine_pool":{"current_model_memory":1000,"final_ceiling":2000}}"#,
        ),
        _ => (404, r#"{"detail":"Not Found"}"#),
    });
    let home = TempDir::new("omlx-found-later");
    let mut client = discovering(port, &home, "");
    // A wait left over from a runtime that timed out earlier.
    client.next_poll = Instant::now() + Duration::from_secs(30);
    client.retry_backoff = Duration::from_secs(30);
    for _ in 0..3 {
        assert!(client.poll(None).is_none());
    }
    let telemetry = client.poll(Some("oMLX")).expect("the oMLX found is asked");
    assert_eq!(telemetry.model.as_deref(), Some("qwen"));
    let report = client.report();
    assert_eq!(report.provider.as_deref(), Some("oMLX"));
    assert_eq!(report.selection, "process detection; oMLX endpoint config");
    assert!(report.connected);
    assert_eq!(client.retry_backoff, Duration::from_secs(1));
    // Health alone: without a key nothing else is asked.
    assert_eq!(stop(port, server), ["GET /health HTTP/1.1"]);
}
