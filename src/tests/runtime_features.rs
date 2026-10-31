use super::*;
use serde_json::json;

#[test]
fn optional_splash_features_name_storage_and_ane_without_inventing_rates() {
    let status = json!({
        "streamed_experts":{"arena_bytes":1048576,"prefill":{"read_bytes":1024},"decode":{"read_bytes":2048},"failure":""},
        "streamed_table":{"cache_bytes":2048,"read_bytes":4096,"failure":"read failed"},
        "kv_spill":{"pages":{"spilled_bytes":8192},"requests":{"waiting_quota":2}},
        "ane_ffn":{"state":"stopped","share":0.25,"reruns":3,"reason":"timed out"},
        "verify_rows":"auto","cache":{"deduplicated_kv_tokens":128},
        "admission":{"suspended":1,"waiting_memory":2,"waiting_concurrency":3}
    });
    let details = splash_details(&status).join(" / ");
    for expected in [
        "SSD experts",
        "RAM 1.0 MiB",
        "read 3.0 KiB total",
        "SSD PLE",
        "read failed",
        "KV spill",
        "2 waiting for SSD quota",
        "ANE stopped",
        "25% FFN share",
        "3 GPU reruns",
        "timed out",
        "draft verification auto",
        "128 tokens total",
        "1 suspended",
        "3 waiting for concurrency",
    ] {
        assert!(details.contains(expected), "missing {expected}: {details}");
    }
    assert!(!details.contains("tok/s"));
    assert!(splash_details(&json!({})).is_empty());
    assert!(splash_details(&json!({"ane_ffn":{"share":3},"streamed_experts":null})).is_empty());
}

#[test]
fn omlx_settings_are_scoped_to_loaded_models_and_labeled_configuration() {
    let stats = json!({"active_models":{"models":[{"id":"busy"}]}});
    let settings = json!({"models":[
        {"id":"busy","settings":{"gpu_power_limit_w":10,"mtp_enabled":true,"dflash_enabled":true,"vlm_mtp_enabled":true,"moe_expert_offload_enabled":true}},
        {"id":"unloaded","settings":{"gpu_power_limit_w":5}}
    ]});
    let details = omlx_details(&stats, Some(&settings)).join(" / ");
    assert!(details.contains("GPU cap configured 10 W (application unverified)"));
    for mode in ["Lightning MTP", "DFlash", "VLM MTP", "expert offload"] {
        assert!(details.contains(&format!("{mode} configured")));
    }
    assert!(!details.contains("unloaded"));
    assert!(omlx_details(&json!({}), None).is_empty());
    assert!(omlx_details(&stats, None).is_empty());
    let inline =
        json!({"active_models":{"models":[{"id":"busy","settings":{"gpu_power_limit_w":0}}]}});
    assert!(omlx_details(&inline, None).is_empty());
    let mut details = None;
    append_details(&mut details, vec![]);
    assert!(details.is_none());
    append_details(&mut details, vec!["one".into()]);
    append_details(&mut details, vec!["two".into()]);
    assert_eq!(details.as_deref(), Some("one · two"));
}
