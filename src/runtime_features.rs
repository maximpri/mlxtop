// SPDX-License-Identifier: MIT
//! Optional runtime observations. Missing fields never become measured zeroes.
use crate::formatting::bytes;
use serde_json::Value;

fn count(value: &Value, field: &str) -> Option<u64> {
    value.get(field)?.as_u64()
}

fn text(value: &Value, field: &str) -> Option<String> {
    let text: String = value
        .get(field)?
        .as_str()?
        .chars()
        .filter(|c| !c.is_control())
        .take(160)
        .collect();
    (!text.is_empty()).then_some(text)
}

pub(crate) fn splash_details(status: &Value) -> Vec<String> {
    let mut details = Vec::new();
    for (field, label, memory) in [
        ("streamed_experts", "SSD experts", "arena_bytes"),
        ("streamed_table", "SSD PLE", "cache_bytes"),
    ] {
        if let Some(store) = status.get(field).filter(|v| v.is_object()) {
            let mut summary = label.to_owned();
            if let Some(size) = count(store, memory) {
                summary.push_str(&format!(" · RAM {}", bytes(size)));
            }
            let reads = count(store, "read_bytes").or_else(|| {
                count(store.get("prefill")?, "read_bytes")?
                    .checked_add(count(store.get("decode")?, "read_bytes")?)
            });
            if let Some(reads) = reads {
                summary.push_str(&format!(" · read {} total", bytes(reads)));
            }
            if let Some(failure) = text(store, "failure") {
                summary.push_str(&format!(" · {failure}"));
            }
            details.push(summary);
        }
    }
    if let Some(spill) = status.get("kv_spill").filter(|v| v.is_object()) {
        let mut summary = "KV spill".to_owned();
        if let Some(size) = spill
            .pointer("/pages/spilled_bytes")
            .and_then(Value::as_u64)
        {
            summary.push_str(&format!(" · {} on SSD", bytes(size)));
        }
        if let Some(waiting) = spill
            .pointer("/requests/waiting_quota")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0)
        {
            summary.push_str(&format!(" · {waiting} waiting for SSD quota"));
        }
        details.push(summary);
    }
    if let Some(ane) = status.get("ane_ffn").filter(|v| v.is_object()) {
        if let Some(state) = text(ane, "state") {
            let mut summary = format!("ANE {state}");
            if let Some(share) = ane
                .get("share")
                .and_then(Value::as_f64)
                .filter(|v| (0.0..=1.0).contains(v))
            {
                summary.push_str(&format!(" · {:.0}% FFN share", share * 100.0));
            }
            if let Some(reruns) = count(ane, "reruns").filter(|n| *n > 0) {
                summary.push_str(&format!(" · {reruns} GPU reruns"));
            }
            if let Some(reason) = text(ane, "reason") {
                summary.push_str(&format!(" · {reason}"));
            }
            details.push(summary);
        }
    }
    if let Some(mode) = text(status, "verify_rows") {
        details.push(format!("draft verification {mode}"));
    }
    if let Some(n) = status
        .pointer("/cache/deduplicated_kv_tokens")
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
    {
        details.push(format!("KV deduplicated {n} tokens total"));
    }
    for (field, label) in [
        ("suspended", "suspended"),
        ("waiting_concurrency", "waiting for concurrency"),
    ] {
        if let Some(n) = status
            .pointer(&format!("/admission/{field}"))
            .and_then(Value::as_u64)
            .filter(|n| *n > 0)
        {
            details.push(format!("{n} {label}"));
        }
    }
    details
}

/// Settings describe configuration, not proof the hardware cap or a draft ran.
/// Only currently loaded/active models are included; catalogue entries aren't serving.
pub(crate) fn omlx_details(stats: &Value, model_settings: Option<&Value>) -> Vec<String> {
    let mut details = Vec::new();
    let Some(active) = stats
        .pointer("/active_models/models")
        .and_then(Value::as_array)
    else {
        return details;
    };
    for model in active {
        let Some(id) = model.get("id").and_then(Value::as_str) else {
            continue;
        };
        let settings = model.get("settings").or_else(|| {
            model_settings?
                .get("models")?
                .as_array()?
                .iter()
                .find(|m| m.get("id").and_then(Value::as_str) == Some(id))?
                .get("settings")
        });
        let label = text(model, "id").unwrap_or_default();
        if let Some(settings) = settings {
            if let Some(watts) =
                count(settings, "gpu_power_limit_w").filter(|w| (2..=1000).contains(w))
            {
                details.push(format!(
                    "{label}: GPU cap configured {watts} W (application unverified)"
                ));
            }
            for (field, name) in [
                ("dflash_enabled", "DFlash"),
                ("mtp_enabled", "Lightning MTP"),
                ("vlm_mtp_enabled", "VLM MTP"),
            ] {
                if settings.get(field).and_then(Value::as_bool) == Some(true) {
                    details.push(format!("{label}: {name} configured"));
                }
            }
            if settings
                .get("moe_expert_offload_enabled")
                .and_then(Value::as_bool)
                == Some(true)
            {
                details.push(format!("{label}: expert offload configured"));
            }
        }
    }
    details
}

pub(crate) fn append_details(details: &mut Option<String>, additional: Vec<String>) {
    if additional.is_empty() {
        return;
    }
    let suffix = additional.join(" · ");
    match details {
        Some(existing) if !existing.is_empty() => {
            existing.push_str(" · ");
            existing.push_str(&suffix);
        }
        _ => *details = Some(suffix),
    }
}

#[cfg(test)]
#[path = "tests/runtime_features.rs"]
mod tests;
